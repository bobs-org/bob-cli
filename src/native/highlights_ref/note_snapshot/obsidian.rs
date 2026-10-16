//! Obsidian-layer syntax projection: vault Markdown into view-model blocks.
//!
//! This module turns one note's cached source bytes into [`Block`] and
//! [`Inline`] nodes, with no renderer or subprocess dependency. It works
//! from cached source text and emits model nodes, using source offsets and
//! the existing fence/wikilink primitives rather than successive
//! whole-document regex substitutions. Ordinary Markdown structure,
//! including lists and code, is preserved; user inline structure is parsed
//! far enough to keep emphasis, links, and their escaping in the model.
//!
//! Protection rules (literal text never becomes syntax):
//!
//! - Fenced code (any fence character or length) is never rewritten.
//! - Inline code spans are never rewritten, except that a span whose
//!   content is an inline query (`` `= ...` ``) becomes a linked
//!   live-query placeholder.
//! - Inline `%%...%%` pairs, multiline `%%` spans, and `<!--...-->` HTML
//!   comments are dropped outside protected code only.
//!
//! Links, images, and embeds resolve through the shared [`SourceStore`]
//! cache, so the root note, block excerpts, and later transclusion reads
//! all share one immutable copy of every source file. Note embeds
//! (`![[note]]`, `![[note#Heading]]`, `![[note#^id]]`) are returned both
//! as empty [`Block::Embed`] placeholders and as [`PendingEmbed`] requests
//! carrying resolution, origin, and fragment: the transclusions phase owns
//! expansion, cycle/size guards, and fallback diagnostics. This phase never
//! expands an embed and never follows a plain link for inclusion.
//!
//! Checkboxes (`- [c]`) are recognized with or without any task tag and
//! map through the vault's Tasks settings to the six model statuses, with
//! `*` Next, `/` In Progress, and `?` Blocked distinguished explicitly.
//! Unknown symbols stay visible with an [`SnapshotWarningKind::UnknownTaskStatus`]
//! warning and an Open fallback. Closed-task filtering belongs to the
//! later Bob layer. Raw task bodies, frontmatter, extents, indentation,
//! and block IDs are retained in [`ParsedSource`] for Bob-layer parsing
//! without reopening source files.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::classify::{SourceLoader, SourceStore};
use super::resolve::{
    build_fragment_index, obsidian_uri, obsidian_uri_with_fragment,
    parse_attachment_target, parse_link_target, width_hint_to_percent,
    AttachmentIndex, AttachmentResolutionError, FragmentIndex, FragmentLookup,
    LinkFragment, NoteResolveError, ParsedLinkTarget, ResolvedAttachment,
    ResolvedNote, VaultNotes,
};
use super::view::{
    AnchorIdentity, AnchorKind, Block, Chip, ChipKind, HeadingClass,
    HeadingLevel, Inline, LinkTarget, ListItem, ListKind, ParsedSource,
    ParsedTaskLine, SnapshotAnchor, SnapshotAsset, SnapshotHeading,
    SnapshotTask, SnapshotTaskStatus, SnapshotWarning, SnapshotWarningKind,
    SourceExtent, SourceFrontmatter, SourceIdentity, SourceIndentation,
    SourceLocation,
};
use crate::native::collect_done::trailing_block_id_in_line;
use crate::native::markdown::{
    atx_heading, closes_fence, fence_marker, fenced_lines,
    multiline_comment_lines, strictly_closed_frontmatter_end,
};
use crate::native::task_status_hooks::{TaskStatusType, TasksSettings};

/// Field keys dropped as hidden bookkeeping. Matching is case-sensitive,
/// mirroring the shared inline-field scanner. Every other bracketed or
/// legacy field is preserved as readable muted `key: value` text.
const BOOKKEEPING_FIELDS: &[&str] = &[
    "fresh",
    "created",
    "id",
    "dependsOn",
    "keeps",
    "unblocked",
    "h",
    "gkeep",
];

/// Structural tags dropped from visible text. `#hide` never suppresses a
/// row. Every other tag is retained as muted text. Matching is exact
/// lowercase: an unfamiliar capitalization stays visible rather than being
/// deleted.
const STRUCTURAL_TAGS: &[&str] =
    &["#task", "#ref", "#prj", "#hide", "#prj_is_blocked"];

/// Fence info names (first word, case-insensitive) treated as live-query
/// placeholders instead of code. Queries are never executed.
const QUERY_FENCE_NAMES: &[&str] =
    &["dataview", "dataviewjs", "tasks", "task", "base"];

/// A word this long with no whitespace becomes a code span so the
/// code-break filter can wrap it. Bare URLs are autolinked first, so this
/// only catches non-URL tokens such as digests and long paths.
const LONG_TOKEN_CHARS: usize = 40;

/// Excerpts for block-reference labels are capped to this display width so
/// one long task cannot dominate a link label.
const EXCERPT_CHARS: usize = 140;

/// One note embed awaiting the transclusions phase. The matching
/// [`Block::Embed`] in [`SyntaxProjection::blocks`] carries the same
/// readable `label` with empty contents; the transclusions phase fills it
/// in one level deep, enforcing cycle and size guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingEmbed {
    /// Readable source label, e.g. `Route › Heading` or `Route`.
    pub label: String,
    /// Canonical path of the resolved target note.
    pub canonical_path: PathBuf,
    /// Vault-relative path of the resolved target note.
    pub vault_relative: PathBuf,
    /// Requested heading/block fragment, if any.
    pub fragment: Option<LinkFragment>,
    /// One-based host line that requested the embed.
    pub origin_line: Option<usize>,
    /// True when the embed addresses its own host note.
    pub same_note: bool,
}

/// The first original H1 outside fences and comments, exposed so the
/// caller can apply the title-selection rule (remove a consumed title
/// heading). The level is the source level, never the shifted display
/// level, so a normalized H2 is never mistaken for the original H1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TitleCandidate {
    /// Plain display text of the heading.
    pub display_text: String,
    /// Deterministic collision-safe anchor id of the heading.
    pub anchor_id: String,
}

/// The complete syntax-layer result for one note.
#[derive(Debug)]
pub(crate) struct SyntaxProjection {
    /// Lossless source record for the Bob layer.
    pub parsed: ParsedSource,
    /// Projected body blocks. Embeds are empty placeholders; see
    /// [`PendingEmbed`].
    pub blocks: Vec<Block>,
    /// Embed expansion requests for the transclusions phase.
    pub pending_embeds: Vec<PendingEmbed>,
    /// Deterministic, document-order diagnostics with origin lines.
    pub warnings: Vec<SnapshotWarning>,
    /// Emitted heading and `t-ID` anchors in document order.
    pub anchors: Vec<SnapshotAnchor>,
    /// First original H1, when the note has one.
    pub title_candidate: Option<TitleCandidate>,
    /// Registered image assets in first-use order.
    pub assets: Vec<SnapshotAsset>,
    /// Wikilink/Markdown-link resolutions (embeds excluded).
    pub links_resolved: u64,
    /// Unresolved content links (embeds excluded).
    pub links_unresolved: u64,
}

/// Map one checkbox symbol to its model status. `*` Next, `/` In Progress,
/// and `?` Blocked win explicitly over settings; remaining symbols fall
/// back to the vault's Tasks `status_types`. Returns `None` for unknown
/// symbols, which callers keep visible with a warning and an Open
/// fallback. Task status is never derived or written here.
pub(crate) fn status_for_symbol(
    symbol: char,
    settings: &TasksSettings,
) -> Option<SnapshotTaskStatus> {
    match symbol {
        ' ' => Some(SnapshotTaskStatus::Open),
        'x' | 'X' => Some(SnapshotTaskStatus::Done),
        '*' => Some(SnapshotTaskStatus::Next),
        '/' => Some(SnapshotTaskStatus::Wip),
        '?' => Some(SnapshotTaskStatus::Blocked),
        '-' => Some(SnapshotTaskStatus::Canceled),
        _ => settings.status_types.get(&symbol).map(|status_type| {
            match status_type {
                TaskStatusType::Todo => SnapshotTaskStatus::Open,
                TaskStatusType::Done => SnapshotTaskStatus::Done,
                TaskStatusType::InProgress => SnapshotTaskStatus::Wip,
                TaskStatusType::OnHold => SnapshotTaskStatus::Blocked,
                TaskStatusType::Cancelled => SnapshotTaskStatus::Canceled,
                TaskStatusType::NonTask | TaskStatusType::Empty => {
                    SnapshotTaskStatus::Open
                }
            }
        }),
    }
}

/// Project one note's Obsidian syntax into view-model blocks.
///
/// `host` identifies the note; its bytes are read through `store` so the
/// root digest, excerpts, and transclusion reads share one cached copy.
/// Callers pass a fresh [`AttachmentIndex`] per projection operation (the
/// transclusions phase reuses it while expanding embeds). Image widths are
/// parsed from `|px` hints and converted on the shared print measure.
///
/// Note embeds are not expanded: each becomes an empty [`Block::Embed`]
/// plus a [`PendingEmbed`] request. Callers assemble sections and the
/// final anchor set; internal links already carry structured
/// [`LinkTarget::Internal`] identities for that step.
pub(crate) fn project_obsidian_syntax<L: SourceLoader>(
    store: &mut SourceStore<L>,
    attachments: &mut AttachmentIndex,
    host: &SourceIdentity,
    settings: &TasksSettings,
) -> Result<SyntaxProjection, String> {
    let bytes = store.read_canonical(&host.canonical_path)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let projector = Projector::new(store, attachments, host, &text, settings);
    projector.run(&bytes)
}

/// Project a selected extent from an already cached source note. The full source is
/// still used for same-note fragment checks, while only `selected_text` becomes body
/// content. Leading newlines preserve original diagnostic line numbers and keep a
/// selected `---` line from being mistaken for frontmatter.
pub(crate) fn project_obsidian_syntax_selection<L: SourceLoader>(
    store: &mut SourceStore<L>,
    attachments: &mut AttachmentIndex,
    host: &SourceIdentity,
    full_source_bytes: &[u8],
    selected_text: &str,
    first_line: usize,
    settings: &TasksSettings,
) -> Result<SyntaxProjection, String> {
    let full_text = String::from_utf8_lossy(full_source_bytes).into_owned();
    let mut selected = "\n".repeat(first_line.saturating_sub(1));
    selected.push_str(selected_text);
    let projector =
        Projector::new(store, attachments, host, &full_text, settings);
    projector.run(selected.as_bytes())
}

/// One preprocessed source line. `text` is comment-stripped for ordinary
/// lines and byte-identical for fenced lines; `raw` is always the original
/// line for task-record fidelity.
struct WorkLine {
    /// Zero-based index into the source line table.
    index: usize,
    /// One-based line number.
    line_no: usize,
    /// Byte offset of the line start in the source text.
    byte_start: usize,
    /// Original line without its terminator.
    raw: String,
    /// Prefix of the original line removed while projecting blockquote/callout
    /// structure. It maps the effective syntax back to the original source line.
    source_prefix: String,
    /// Effective line content after comment stripping.
    text: String,
    /// True for lines inside (or bounding) a fenced code block.
    fenced: bool,
    /// True for frontmatter and comment-hidden lines: never projected.
    dropped: bool,
}

impl WorkLine {
    fn indent_width(&self) -> usize {
        indent_width(&self.text)
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Mutable projection state threaded through block and inline parsing.
struct Projector<'a, L: SourceLoader> {
    store: &'a mut SourceStore<L>,
    attachments: &'a mut AttachmentIndex,
    host: &'a SourceIdentity,
    host_relative: PathBuf,
    vault_name: String,
    vault_root: PathBuf,
    vault: VaultNotes,
    host_file_uri: String,
    settings: &'a TasksSettings,
    host_index: FragmentIndex,
    fragment_cache: HashMap<PathBuf, Option<FragmentIndex>>,
    used_anchors: HashSet<String>,
    anchors: Vec<SnapshotAnchor>,
    warnings: Vec<SnapshotWarning>,
    pending_embeds: Vec<PendingEmbed>,
    assets: Vec<SnapshotAsset>,
    asset_paths: HashSet<PathBuf>,
    links_resolved: u64,
    links_unresolved: u64,
    task_lines: Vec<ParsedTaskLine>,
    title_candidate: Option<TitleCandidate>,
    seen_title: bool,
    min_heading_level: usize,
}

impl<'a, L: SourceLoader> Projector<'a, L> {
    fn new(
        store: &'a mut SourceStore<L>,
        attachments: &'a mut AttachmentIndex,
        host: &'a SourceIdentity,
        host_index_text: &str,
        settings: &'a TasksSettings,
    ) -> Self {
        let vault_name = store.vault_name().to_string();
        let vault_root = store.vault_root().to_path_buf();
        let host_relative =
            PathBuf::from(host.vault_relative_path.replace('\\', "/"));
        let host_file_uri =
            obsidian_uri(&vault_name, &host.vault_relative_path);
        let vault = VaultNotes::new(vault_root.clone(), vault_name.clone());
        let host_index = build_fragment_index(host_index_text);
        Self {
            store,
            attachments,
            host,
            host_relative,
            vault_name,
            vault_root,
            vault,
            host_file_uri,
            settings,
            host_index,
            fragment_cache: HashMap::new(),
            used_anchors: HashSet::new(),
            anchors: Vec::new(),
            warnings: Vec::new(),
            pending_embeds: Vec::new(),
            assets: Vec::new(),
            asset_paths: HashSet::new(),
            links_resolved: 0,
            links_unresolved: 0,
            task_lines: Vec::new(),
            title_candidate: None,
            seen_title: false,
            min_heading_level: 1,
        }
    }

    fn run(mut self, bytes: &[u8]) -> Result<SyntaxProjection, String> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let lines = split_source_lines(&text);
        let frontmatter = parse_frontmatter_record(&lines);
        let body_start =
            frontmatter.as_ref().map_or(0, |front| front.end_index + 1);
        let work = preprocess_lines(&lines, body_start);
        self.min_heading_level = shallowest_heading_level(&work);
        let blocks = self.parse_blocks(&work, 0);
        let parsed = ParsedSource::from_bytes(
            self.host.clone(),
            bytes.to_vec(),
            frontmatter.map(|front| front.record),
            std::mem::take(&mut self.task_lines),
        );
        Ok(SyntaxProjection {
            parsed,
            blocks,
            pending_embeds: std::mem::take(&mut self.pending_embeds),
            warnings: std::mem::take(&mut self.warnings),
            anchors: std::mem::take(&mut self.anchors),
            title_candidate: self.title_candidate.take(),
            assets: std::mem::take(&mut self.assets),
            links_resolved: self.links_resolved,
            links_unresolved: self.links_unresolved,
        })
    }

    fn warn(
        &mut self,
        kind: SnapshotWarningKind,
        line_no: Option<usize>,
        message: String,
    ) {
        self.warnings.push(SnapshotWarning {
            kind,
            origin: Some(SourceLocation {
                source: self.host.clone(),
                line: line_no,
            }),
            message,
        });
    }

    /// Claim a deterministic collision-safe anchor id (`dup`, `dup-2`, ...).
    fn claim_anchor_id(&mut self, base: &str) -> String {
        let base = if base.is_empty() {
            "section".to_string()
        } else {
            base.to_string()
        };
        if self.used_anchors.insert(base.clone()) {
            return base;
        }
        let mut counter = 2_u32;
        loop {
            let candidate = format!("{base}-{counter}");
            if self.used_anchors.insert(candidate.clone()) {
                return candidate;
            }
            counter += 1;
        }
    }

    fn push_anchor(
        &mut self,
        kind: AnchorKind,
        fragment: String,
        id: String,
    ) -> SnapshotAnchor {
        let anchor = SnapshotAnchor {
            identity: AnchorIdentity {
                source: self.host.clone(),
                kind,
                fragment,
            },
            id,
        };
        self.anchors.push(anchor.clone());
        anchor
    }
}

/// One line of the source text with its byte offset.
struct SourceLine<'a> {
    text: &'a str,
    byte_start: usize,
}

fn split_source_lines(text: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for chunk in text.split_inclusive('\n') {
        let mut line = chunk;
        if let Some(stripped) = line.strip_suffix('\n') {
            line = stripped;
        }
        if let Some(stripped) = line.strip_suffix('\r') {
            line = stripped;
        }
        lines.push(SourceLine {
            text: line,
            byte_start: offset,
        });
        offset += chunk.len();
    }
    // `str::lines` semantics drop a trailing empty chunk; mirror that so a
    // missing trailing newline needs no special case downstream.
    if text.ends_with('\n')
        && lines.last().is_some_and(|line| line.text.is_empty())
    {
        lines.pop();
    }
    lines
}

struct FrontmatterSpan {
    record: SourceFrontmatter,
    end_index: usize,
}

fn parse_frontmatter_record(
    lines: &[SourceLine<'_>],
) -> Option<FrontmatterSpan> {
    let borrowed = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let end = strictly_closed_frontmatter_end(&borrowed)?;
    let yaml = borrowed[1..end].join("\n");
    let metadata =
        serde_yaml::from_str(&yaml).unwrap_or(serde_yaml::Value::Null);
    let byte_end = if end + 1 < lines.len() {
        lines[end + 1].byte_start
    } else {
        lines[end].byte_start + lines[end].text.len() + 1
    };
    Some(FrontmatterSpan {
        record: SourceFrontmatter {
            extent: SourceExtent {
                byte_start: 0,
                byte_end,
                first_line: 1,
                last_line: end + 1,
            },
            raw_yaml: yaml,
            metadata,
        },
        end_index: end,
    })
}

/// Strip comments and mark hidden lines. Fenced code lines pass through
/// untouched; frontmatter lines are dropped; multiline `%%` and HTML
/// comment spans hide the lines they cover while keeping text around
/// same-line pairs.
fn preprocess_lines(
    lines: &[SourceLine<'_>],
    body_start: usize,
) -> Vec<WorkLine> {
    let borrowed = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let fenced = fenced_lines(&borrowed, body_start..borrowed.len());
    let mut state = CommentState::default();
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index < body_start {
                return WorkLine {
                    index,
                    line_no: index + 1,
                    byte_start: line.byte_start,
                    raw: line.text.to_string(),
                    source_prefix: String::new(),
                    text: String::new(),
                    fenced: false,
                    dropped: true,
                };
            }
            if fenced.contains(&index) {
                return WorkLine {
                    index,
                    line_no: index + 1,
                    byte_start: line.byte_start,
                    raw: line.text.to_string(),
                    source_prefix: String::new(),
                    text: line.text.to_string(),
                    fenced: true,
                    dropped: false,
                };
            }
            match strip_line_comments(line.text, &mut state) {
                Some(text) => WorkLine {
                    index,
                    line_no: index + 1,
                    byte_start: line.byte_start,
                    raw: line.text.to_string(),
                    source_prefix: String::new(),
                    text,
                    fenced: false,
                    dropped: false,
                },
                None => WorkLine {
                    index,
                    line_no: index + 1,
                    byte_start: line.byte_start,
                    raw: line.text.to_string(),
                    source_prefix: String::new(),
                    text: String::new(),
                    fenced: false,
                    dropped: true,
                },
            }
        })
        .collect()
}

/// Line numbers hidden by the same fenced/inline-code-aware comment scanner used by
/// syntax projection. Fragment indexing calls this so comment-looking code never
/// hides later headings.
pub(super) fn hidden_comment_lines(lines: &[&str]) -> BTreeSet<usize> {
    let fenced = fenced_lines(lines, 0..lines.len());
    let mut state = CommentState::default();
    let mut hidden = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if fenced.contains(&index) {
            continue;
        }
        if strip_line_comments(line, &mut state).is_none() {
            hidden.insert(index);
        }
    }
    hidden
}

/// Produce one heading line's visible spelling with syntax projection's comment
/// rules, including protection for inline code spans.
pub(super) fn strip_comments_for_fragment(line: &str) -> String {
    strip_line_comments(line, &mut CommentState::default()).unwrap_or_default()
}

#[derive(Default)]
struct CommentState {
    in_percent: bool,
    in_html: bool,
}

/// Strip one line's comments outside inline code. Returns `None` when the
/// whole line is hidden inside a multiline span. Inline code spans are
/// recomputed after every removal so offsets stay honest.
fn strip_line_comments(line: &str, state: &mut CommentState) -> Option<String> {
    let mut current = line;
    let mut visible = String::new();
    loop {
        if state.in_percent {
            let Some(position) = find_outside_code(current, "%%") else {
                return (!visible.is_empty()).then_some(visible);
            };
            current = &current[position + 2..];
            state.in_percent = false;
        } else if state.in_html {
            let Some(position) = find_outside_code(current, "-->") else {
                return (!visible.is_empty()).then_some(visible);
            };
            current = &current[position + 3..];
            state.in_html = false;
        } else {
            let percent = find_outside_code(current, "%%");
            let html = find_outside_code(current, "<!--");
            let percent_first = match (percent, html) {
                (Some(open), None) => Some(open),
                (Some(open), Some(other)) if other > open => Some(open),
                _ => None,
            };
            if let Some(open) = percent_first {
                visible.push_str(&current[..open]);
                match find_outside_code_from(current, "%%", open + 2) {
                    Some(close) => {
                        current = &current[close + 2..];
                    }
                    None => {
                        state.in_percent = true;
                        return (!visible.is_empty()).then_some(visible);
                    }
                }
            } else if let Some(open) = html {
                visible.push_str(&current[..open]);
                match find_outside_code_from(current, "-->", open + 4) {
                    Some(close) => {
                        current = &current[close + 3..];
                    }
                    None => {
                        state.in_html = true;
                        return (!visible.is_empty()).then_some(visible);
                    }
                }
            } else {
                visible.push_str(current);
                return Some(visible);
            }
        }
    }
}

/// Find `needle` outside inline code spans, or `None`.
fn find_outside_code(haystack: &str, needle: &str) -> Option<usize> {
    find_outside_code_from(haystack, needle, 0)
}

fn find_outside_code_from(
    haystack: &str,
    needle: &str,
    from: usize,
) -> Option<usize> {
    let spans = inline_code_spans(&haystack[from.min(haystack.len())..]);
    let mut search = from;
    while let Some(position) = haystack[search..].find(needle) {
        let absolute = search + position;
        let inside = spans.iter().any(|(start, end)| {
            *start + from <= absolute && absolute < *end + from
        });
        if !inside {
            return Some(absolute);
        }
        search = absolute + 1;
    }
    None
}

/// Byte ranges of inline code spans on one line (CommonMark rules: a
/// closer must repeat the opener's exact backtick count; an unclosed run
/// is literal text). Escaped backticks never open a span.
fn inline_code_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' || is_escaped(line, index) {
            index += 1;
            continue;
        }
        let open_start = index;
        while index < bytes.len() && bytes[index] == b'`' {
            index += 1;
        }
        let run = index - open_start;
        let mut cursor = index;
        let mut closed = None;
        while cursor < bytes.len() {
            if bytes[cursor] == b'`' && !is_escaped(line, cursor) {
                let close_start = cursor;
                while cursor < bytes.len() && bytes[cursor] == b'`' {
                    cursor += 1;
                }
                if cursor - close_start == run {
                    closed = Some(close_start + (cursor - close_start));
                    break;
                }
                continue;
            }
            cursor += 1;
        }
        match closed {
            Some(close_end) => {
                spans.push((open_start, close_end));
                index = close_end;
            }
            None => {
                index = open_start + 1;
            }
        }
    }
    spans
}

fn is_escaped(line: &str, position: usize) -> bool {
    let bytes = line.as_bytes();
    let mut backslashes = 0;
    let mut cursor = position;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

/// Tab-aware display width of leading whitespace (tabs stop every 4).
fn indent_width(line: &str) -> usize {
    const TAB_STOP: usize = 4;
    let mut width = 0;
    for character in line.chars() {
        if character == ' ' {
            width += 1;
        } else if character == '\t' {
            width += TAB_STOP - (width % TAB_STOP);
        } else {
            break;
        }
    }
    width
}

/// Literal leading whitespace prefix of a line.
fn indent_prefix(line: &str) -> &str {
    let end = line
        .find(|character: char| character != ' ' && character != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

/// Shallowest ATX level over projected (kept, non-fenced) lines.
fn shallowest_heading_level(lines: &[WorkLine]) -> usize {
    lines
        .iter()
        .filter(|line| !line.dropped && !line.fenced)
        .filter_map(|line| atx_heading(&line.text).map(|(level, _)| level))
        .min()
        .unwrap_or(1)
}

/// Shift an original ATX level so the shallowest body level reads H1,
/// capping deeper levels at H3 (the model's deepest level).
fn shifted_level(original: usize, minimum: usize) -> HeadingLevel {
    match original.saturating_sub(minimum.saturating_sub(1)).min(3) {
        1 => HeadingLevel::H1,
        2 => HeadingLevel::H2,
        _ => HeadingLevel::H3,
    }
}

/// One parsed checkbox marker: literal indent, status symbol, and the raw
/// body after the marker (possibly empty).
struct TaskMarker<'a> {
    indent: &'a str,
    symbol: char,
    body: &'a str,
}

/// Match `- [c]` (body optional) with any space/tab indent. Only the `-`
/// marker counts as a task row; `*`/`+` markers stay list items.
fn parse_task_marker(line: &str) -> Option<TaskMarker<'_>> {
    let indent_end = line
        .find(|character: char| character != ' ' && character != '\t')
        .unwrap_or(line.len());
    let rest = line[indent_end..].strip_prefix("- [")?;
    let mut chars = rest.chars();
    let symbol = chars.next()?;
    let after = &rest[symbol.len_utf8()..];
    let tail = after.strip_prefix(']')?;
    let body = tail
        .strip_prefix(' ')
        .unwrap_or_else(|| tail.strip_prefix('\t').unwrap_or(tail));
    // `- []` (empty symbol) is a list item, never a task row.
    if tail.is_empty() || tail.starts_with(' ') || tail.starts_with('\t') {
        Some(TaskMarker {
            indent: &line[..indent_end],
            symbol,
            body,
        })
    } else {
        None
    }
}

/// One parsed list marker and the content after it.
struct ListMarker<'a> {
    ordered: bool,
    start: u64,
    content: &'a str,
    width: usize,
}

fn parse_list_marker(line: &str) -> Option<ListMarker<'_>> {
    let stripped = line.trim_start();
    let prefix_len = line.len() - stripped.len();
    for marker in ["- ", "* ", "+ "] {
        if let Some(content) = stripped.strip_prefix(marker) {
            return Some(ListMarker {
                ordered: false,
                start: 1,
                content,
                width: prefix_len + 2,
            });
        }
    }
    let digits = stripped.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &stripped[digits..];
        for closer in [". ", ") "] {
            if let Some(content) = rest.strip_prefix(closer) {
                return Some(ListMarker {
                    ordered: true,
                    start: stripped[..digits].parse().unwrap_or(1),
                    content,
                    width: prefix_len + digits + 2,
                });
            }
        }
    }
    None
}

/// True for a thematic-break line (`---`, `***`, `___` and longer runs).
fn is_thematic_break(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let mut chars = trimmed.chars();
    let marker = chars.next().expect("non-empty");
    if !matches!(marker, '-' | '*' | '_') {
        return false;
    }
    chars.all(|character| character == marker)
}

/// True for a callout line: up to three spaces, then `>`.
fn is_callout_line(line: &str) -> bool {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count().min(4);
    line[spaces..].starts_with('>')
}

/// Strip one callout level (`>` plus a single optional space).
fn strip_callout_prefix(line: &str) -> &str {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count().min(3);
    let rest = &line[spaces..];
    let rest = rest.strip_prefix('>').unwrap_or(rest);
    rest.strip_prefix(' ').unwrap_or(rest)
}

impl<'a, L: SourceLoader> Projector<'a, L> {
    /// Parse a line slice into blocks. `min_indent` is the display-column
    /// floor: lines dedented below it end the current nesting level.
    fn parse_blocks(
        &mut self,
        lines: &[WorkLine],
        min_indent: usize,
    ) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = &lines[index];
            if line.dropped || line.is_blank() {
                index += 1;
                continue;
            }
            if line.fenced {
                let (block, next) = self.parse_fence(lines, index);
                blocks.push(block);
                index = next;
                continue;
            }
            if line.indent_width() < min_indent {
                break;
            }
            if atx_heading(&line.text).is_some() {
                blocks.push(self.parse_heading(line));
                index += 1;
                continue;
            }
            if is_callout_line(&line.text) {
                let (block, next) = self.parse_callout(lines, index);
                blocks.push(block);
                index = next;
                continue;
            }
            if parse_task_marker(&line.text).is_some() {
                let (block, next) = self.parse_task(lines, index);
                blocks.push(block);
                index = next;
                continue;
            }
            if is_thematic_break(&line.text) {
                blocks.push(Block::Paragraph(vec![Inline::Text(
                    line.text.trim().to_string(),
                )]));
                index += 1;
                continue;
            }
            if parse_list_marker(&line.text).is_some() {
                let (list, next) = self.parse_list(lines, index);
                blocks.push(list);
                index = next;
                continue;
            }
            if is_single_embed_line(&line.text) {
                if let Some(block) = self.parse_line_embed(line) {
                    blocks.push(block);
                    index += 1;
                    continue;
                }
            }
            let (block, next) = self.parse_paragraph(lines, index, min_indent);
            if next <= index {
                // Every dispatch path must consume input. An unhandled single-line
                // construct is kept as readable projected inline content.
                blocks.push(Block::Paragraph(
                    self.parse_inlines(&line.text, Some(line.line_no)),
                ));
                index += 1;
                continue;
            }
            blocks.push(block);
            index = next;
        }
        blocks
    }

    /// Whole-line `![[...]]` / `![...](...)`: image, missing-image, note
    /// embed request, or nothing (the line holds more than one token).
    fn parse_line_embed(&mut self, line: &WorkLine) -> Option<Block> {
        let trimmed = line.text.trim();
        if let Some(inner) = trimmed
            .strip_prefix("![[")
            .and_then(|rest| rest.strip_suffix("]]"))
        {
            if inner.contains("[[") || inner.contains("]]") {
                return None;
            }
            return Some(self.project_wikilink_embed(inner, line.line_no));
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            if let Some(image) =
                self.parse_markdown_image_line(rest, line.line_no)
            {
                return Some(image);
            }
        }
        None
    }

    /// A whole-line Markdown image `![alt](src)` where `rest` follows the
    /// opening `!`. Returns `None` unless the line is exactly one image.
    fn parse_markdown_image_line(
        &mut self,
        rest: &str,
        line_no: usize,
    ) -> Option<Block> {
        let (alt, after) = split_balanced(rest, '[', ']')?;
        let (target, tail) = split_balanced(&after, '(', ')')?;
        if !tail.trim().is_empty() {
            return None;
        }
        let label = self.parse_inlines(&alt, Some(line_no));
        let (path, _) = split_image_target(&target);
        if is_url_target(&path) {
            return Some(Block::Paragraph(vec![Inline::Link {
                label,
                target: LinkTarget::Url(path),
            }]));
        }
        let alt = flatten_inlines(&label);
        let attachment = parse_attachment_target(&path);
        Some(self.project_attachment_image(
            &attachment.path,
            attachment.width_px,
            alt,
            Some(line_no),
        ))
    }

    fn parse_heading(&mut self, line: &WorkLine) -> Block {
        let (level, title) =
            atx_heading(&line.text).expect("heading checked by caller");
        let (title, block_id) = strip_trailing_block_id(title);
        let mut content = self.parse_inlines(title, Some(line.line_no));
        let plain = flatten_inlines(&content);
        let slug = self.claim_anchor_id(&slugify(&plain));
        // The anchor identity must match the fragment index key, which is
        // built from the raw title (minus one `%%` pair and block IDs)
        // with formatting intact — never from flattened plain text.
        // (Inline HTML comments stay in the index key by shared-index
        // design; links to such headings warn rather than misresolve.)
        let key = normalize_heading_key_for(title);
        let anchor = self.push_anchor(AnchorKind::Heading, key, slug.clone());
        if let Some(id) = block_id {
            let tid = self.claim_anchor_id(&format!("t-{id}"));
            let block_anchor = self.push_anchor(AnchorKind::Block, id, tid);
            content.push(Inline::Anchor(block_anchor));
        }
        if level == 1 && !self.seen_title {
            self.seen_title = true;
            if !plain.trim().is_empty() {
                self.title_candidate = Some(TitleCandidate {
                    display_text: plain,
                    anchor_id: slug,
                });
            }
        }
        Block::Heading(SnapshotHeading {
            level: shifted_level(level, self.min_heading_level),
            content,
            class: HeadingClass::Normal,
            anchor,
        })
    }

    /// Parse one task row plus its indented children. The row is recorded
    /// byte-for-byte in the lossless source record.
    fn parse_task(
        &mut self,
        lines: &[WorkLine],
        index: usize,
    ) -> (Block, usize) {
        let line = &lines[index];
        let marker =
            parse_task_marker(&line.text).expect("task checked by caller");
        let width = indent_width(marker.indent);
        let (body, block_id) = strip_trailing_block_id(marker.body);
        let status = match status_for_symbol(marker.symbol, self.settings) {
            Some(status) => status,
            None => {
                self.warn(
                    SnapshotWarningKind::UnknownTaskStatus,
                    Some(line.line_no),
                    format!(
                        "unknown task status '[{}]'; showing as open",
                        marker.symbol
                    ),
                );
                SnapshotTaskStatus::Open
            }
        };
        let mut content = self.parse_inlines(body, Some(line.line_no));
        trim_inline_edges(&mut content);
        let anchor = block_id.clone().map(|id| {
            let tid = self.claim_anchor_id(&format!("t-{id}"));
            self.push_anchor(AnchorKind::Block, id, tid)
        });
        let byte_end = line.byte_start + line.raw.len();
        let raw_task_line = line
            .raw
            .get(line.source_prefix.len()..)
            .unwrap_or(&line.raw);
        let raw_marker = parse_task_marker(raw_task_line);
        let raw_body = raw_marker
            .as_ref()
            .map(|raw_marker| raw_marker.body.to_string())
            .unwrap_or_else(|| marker.body.to_string());
        let source_indent = raw_marker
            .as_ref()
            .map(|raw_marker| raw_marker.indent.to_string())
            .unwrap_or_else(|| marker.indent.to_string());
        self.task_lines.push(ParsedTaskLine {
            raw_line: line.raw.clone(),
            body: raw_body,
            extent: SourceExtent {
                byte_start: line.byte_start,
                byte_end,
                first_line: line.line_no,
                last_line: line.line_no,
            },
            indentation: SourceIndentation {
                columns: indent_width(&source_indent),
                prefix: source_indent,
            },
            block_id,
        });
        let (children, next) = self.parse_nested(lines, index + 1, width);
        (
            Block::Task(SnapshotTask {
                status,
                content,
                chips: Vec::new(),
                anchor,
                children,
            }),
            next,
        )
    }

    /// Collect lines strictly deeper than `parent_width` (blank lines only
    /// when deeper content follows) and parse them one level down.
    fn parse_nested(
        &mut self,
        lines: &[WorkLine],
        start: usize,
        parent_width: usize,
    ) -> (Vec<Block>, usize) {
        let mut end = start;
        let mut cursor = start;
        while cursor < lines.len() {
            let line = &lines[cursor];
            if line.dropped {
                cursor += 1;
                end = cursor;
                continue;
            }
            if line.is_blank() {
                // A blank line belongs to the nested run only when the next
                // visible line stays deeper than the parent.
                let deeper_follows = lines[cursor + 1..]
                    .iter()
                    .find(|next| !next.dropped && !next.is_blank())
                    .is_some_and(|next| next.indent_width() > parent_width);
                if deeper_follows {
                    cursor += 1;
                    continue;
                }
                break;
            }
            // Only strictly deeper lines nest: a col-0 fence after a task
            // is a sibling block, while an indented fence stays inside.
            if line.indent_width() > parent_width {
                end = cursor + 1;
                cursor += 1;
                continue;
            }
            break;
        }
        if end <= start {
            return (Vec::new(), start);
        }
        let children = self.parse_blocks(&lines[start..end], parent_width + 1);
        (children, end)
    }

    fn parse_list(
        &mut self,
        lines: &[WorkLine],
        index: usize,
    ) -> (Block, usize) {
        let first =
            parse_list_marker(&lines[index].text).expect("list checked");
        let ordered = first.ordered;
        let start = first.start;
        let base_width = lines[index].indent_width();
        let mut items = Vec::new();
        let mut cursor = index;
        while cursor < lines.len() {
            let line = &lines[cursor];
            if line.dropped || line.is_blank() {
                break;
            }
            if line.fenced || line.indent_width() != base_width {
                break;
            }
            let Some(marker) = parse_list_marker(&line.text) else {
                break;
            };
            if marker.ordered != ordered
                || parse_task_marker(&line.text).is_some()
                || atx_heading(&line.text).is_some()
            {
                break;
            }
            let (item_body, block_id) = strip_trailing_block_id(marker.content);
            let mut content = self.parse_inlines(item_body, Some(line.line_no));
            match block_id {
                Some(id) => {
                    let tid = self.claim_anchor_id(&format!("t-{id}"));
                    let anchor = self.push_anchor(AnchorKind::Block, id, tid);
                    content.push(Inline::Anchor(anchor));
                }
                None => {}
            }
            let width = line.indent_width();
            let (children, next) = self.parse_nested(lines, cursor + 1, width);
            items.push(ListItem { content, children });
            cursor = next;
        }
        let kind = if ordered {
            ListKind::Ordered { start }
        } else {
            ListKind::Unordered
        };
        (Block::List { kind, items }, cursor)
    }

    fn parse_callout(
        &mut self,
        lines: &[WorkLine],
        index: usize,
    ) -> (Block, usize) {
        let mut inner = Vec::new();
        let mut cursor = index;
        while cursor < lines.len() {
            let line = &lines[cursor];
            if line.dropped || line.fenced || !is_callout_line(&line.text) {
                break;
            }
            inner.push(WorkLine {
                index: line.index,
                line_no: line.line_no,
                byte_start: line.byte_start,
                raw: line.raw.clone(),
                source_prefix: {
                    let effective_prefix = line.text.len()
                        - strip_callout_prefix(&line.text).len();
                    let mut prefix = line.source_prefix.clone();
                    prefix.push_str(&line.text[..effective_prefix]);
                    prefix
                },
                text: strip_callout_prefix(&line.text).to_string(),
                fenced: false,
                dropped: false,
            });
            cursor += 1;
        }
        let mut kind = "note".to_string();
        let mut title = Vec::new();
        if let Some(first) = inner.first() {
            if let Some((parsed_kind, parsed_title)) =
                parse_callout_header(&first.text)
            {
                kind = parsed_kind;
                title = self.parse_inlines(&parsed_title, Some(first.line_no));
                inner.remove(0);
            }
        }
        // Drop blank separators the prefix strip left behind at the edges.
        while inner.first().is_some_and(|line| line.is_blank()) {
            inner.remove(0);
        }
        while inner.last().is_some_and(|line| line.is_blank()) {
            inner.pop();
        }
        // Recompute fence runs over the stripped lines so code inside the
        // callout stays literal.
        let inner_borrowed = inner
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();
        let inner_fenced = fenced_lines(&inner_borrowed, 0..inner.len());
        for (position, line) in inner.iter_mut().enumerate() {
            line.fenced = inner_fenced.contains(&position);
        }
        let blocks = self.parse_blocks(&inner, 0);
        (
            Block::Callout {
                kind,
                title,
                blocks,
            },
            cursor,
        )
    }

    /// Fenced code or live-query block from the opening fence line.
    fn parse_fence(
        &mut self,
        lines: &[WorkLine],
        index: usize,
    ) -> (Block, usize) {
        let opener = fence_marker(&lines[index].text).expect("fence checked");
        let info = lines[index].text.trim_start();
        let info = info
            .chars()
            .skip_while(|character| *character == '`' || *character == '~')
            .collect::<String>();
        let language = info.split_whitespace().next().unwrap_or("").to_string();
        let mut cursor = index + 1;
        while cursor < lines.len() {
            if closes_fence(&lines[cursor].text, opener) {
                cursor += 1;
                break;
            }
            cursor += 1;
        }
        let end = if cursor > index + 1
            && cursor <= lines.len()
            && closes_fence(&lines[cursor - 1].text, opener)
        {
            cursor - 1
        } else {
            cursor.min(lines.len())
        };
        let mut text = lines[index + 1..end]
            .iter()
            .map(|line| line.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !lines[index + 1..end].is_empty() {
            text.push('\n');
        }
        if QUERY_FENCE_NAMES.contains(&language.to_ascii_lowercase().as_str()) {
            return (
                Block::LiveQuery {
                    blocks: vec![Block::Paragraph(vec![Inline::Link {
                        label: vec![Inline::Text(
                            "Open live query".to_string(),
                        )],
                        target: LinkTarget::ObsidianUri(
                            self.host_file_uri.clone(),
                        ),
                    }])],
                },
                cursor,
            );
        }
        (
            Block::Code {
                language: (!language.is_empty()).then_some(language),
                text,
            },
            cursor,
        )
    }

    /// Consecutive prose lines become one paragraph. A line ending in two
    /// spaces hard-breaks; otherwise lines join with a space.
    fn parse_paragraph(
        &mut self,
        lines: &[WorkLine],
        index: usize,
        min_indent: usize,
    ) -> (Block, usize) {
        let mut inlines = Vec::new();
        let mut cursor = index;
        while cursor < lines.len() {
            let line = &lines[cursor];
            if line.dropped || line.is_blank() {
                break;
            }
            if line.fenced
                || line.indent_width() < min_indent
                || atx_heading(&line.text).is_some()
                || is_callout_line(&line.text)
                || parse_task_marker(&line.text).is_some()
                || parse_list_marker(&line.text).is_some()
                || is_thematic_break(&line.text)
            {
                break;
            }
            if cursor > index {
                if lines[cursor - 1].text.ends_with("  ") {
                    inlines.push(Inline::Break);
                } else {
                    inlines.push(Inline::Text(" ".to_string()));
                }
            }
            // Trailing block IDs anchor the actual content and remain in the
            // final visible set only while this paragraph survives filtering.
            let (visible, block_id) = strip_trailing_block_id(&line.text);
            if let Some(id) = block_id {
                let tid = self.claim_anchor_id(&format!("t-{id}"));
                let anchor = self.push_anchor(AnchorKind::Block, id, tid);
                inlines.extend(self.parse_inlines(visible, Some(line.line_no)));
                inlines.push(Inline::Anchor(anchor));
            } else {
                inlines.extend(self.parse_inlines(visible, Some(line.line_no)));
            }
            cursor += 1;
        }
        (Block::Paragraph(inlines), cursor)
    }
}

/// Pure, side-effect-free check for a whole-line embed/image token.
/// Mirrors the dispatch in `parse_line_embed` without resolving anything.
fn is_single_embed_line(line: &str) -> bool {
    let trimmed = line.trim();
    if let Some(inner) = trimmed
        .strip_prefix("![[")
        .and_then(|rest| rest.strip_suffix("]]"))
    {
        return !inner.contains("[[") && !inner.contains("]]");
    }
    if let Some(rest) = trimmed.strip_prefix('!') {
        if let Some((_, after)) = split_balanced(rest, '[', ']') {
            if after.starts_with('(') {
                if let Some((_, tail)) = split_balanced(&after, '(', ')') {
                    return tail.trim().is_empty();
                }
            }
        }
    }
    false
}

/// Parse a callout header `[!kind] optional title`.
fn parse_callout_header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let (kind, title) = rest.split_once(']')?;
    let kind = kind.strip_prefix('!')?;
    if kind.is_empty()
        || !kind.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-'
        })
    {
        return None;
    }
    Some((kind.to_ascii_lowercase(), title.trim().to_string()))
}

/// Normalize a heading's plain text into a fragment key shared with the
/// fragment index (trimmed, case-insensitive).
fn normalize_heading_key_for(plain: &str) -> String {
    plain.trim().to_lowercase()
}

/// Slugify plain heading text into a stable id: lowercase alphanumerics,
/// runs of anything else folded to one `-`, edges trimmed.
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = true;
    for character in text.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                slug.push(lower);
            }
            pending_dash = false;
        } else if !pending_dash {
            slug.push('-');
            pending_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("section");
    }
    slug
}

/// Flatten inline content to plain text for slugs, titles, and excerpts.
fn flatten_inlines(inlines: &[Inline]) -> String {
    let mut plain = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => plain.push_str(text),
            Inline::Anchor(_) => {}
            Inline::Link { label, .. } => {
                plain.push_str(&flatten_inlines(label))
            }
            Inline::Emphasis(content)
            | Inline::Strong(content)
            | Inline::Strike(content)
            | Inline::Muted(content) => {
                plain.push_str(&flatten_inlines(content));
            }
            Inline::Chip(chip) => {
                plain.push_str(&flatten_inlines(&chip.content))
            }
            Inline::Image { alt, .. } => plain.push_str(alt),
            Inline::EmbeddedBlock(block) => match block.as_ref() {
                Block::Embed { source, .. } => plain.push_str(source),
                Block::Paragraph(content) | Block::EmptyState(content) => {
                    plain.push_str(&flatten_inlines(content))
                }
                _ => {}
            },
            Inline::Break => plain.push(' '),
        }
    }
    plain
}

/// Split `text` at a balanced bracket pair starting at byte 0.
/// Returns the inner text and the remainder after the closer.
fn split_balanced(
    text: &str,
    open: char,
    close: char,
) -> Option<(String, String)> {
    let mut depth = 0;
    let mut end = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == open {
            depth += 1;
        } else if character == close {
            depth -= 1;
            if depth == 0 {
                end = Some(index);
                break;
            }
        }
    }
    let end = end?;
    Some((
        text[open.len_utf8()..end].to_string(),
        text[end + close.len_utf8()..].to_string(),
    ))
}

/// Split a Markdown image/link target into its path and optional title.
/// Handles `"double"`, `'single'`, and `(paren)` titles.
fn split_image_target(target: &str) -> (String, Option<String>) {
    let target = target.trim();
    if let Some(path) = target.strip_suffix('"') {
        if let Some(bare) = path.rsplit_once(" \"") {
            return (bare.0.trim().to_string(), Some(bare.1.to_string()));
        }
    }
    if let Some(path) = target.strip_suffix('\'') {
        if let Some(bare) = path.rsplit_once(" '") {
            return (bare.0.trim().to_string(), Some(bare.1.to_string()));
        }
    }
    if let Some(path) = target.strip_suffix(')') {
        if let Some(bare) = path.rsplit_once(" (") {
            return (bare.0.trim().to_string(), Some(bare.1.to_string()));
        }
    }
    (target.to_string(), None)
}

/// True for absolute URL targets (including `obsidian://` and `mailto:`).
fn is_url_target(target: &str) -> bool {
    target.contains("://") || target.starts_with("mailto:")
}

/// Split a trailing `^block-id` token off line content. Returns the
/// visible text and the id (without `^`) when present.
fn strip_trailing_block_id(text: &str) -> (&str, Option<String>) {
    let id = trailing_block_id_in_line(text);
    match &id {
        Some(id) => {
            let trimmed = text.trim_end();
            let without = trimmed[..trimmed.len() - id.len() - 1].trim_end();
            (without, Some(id.clone()))
        }
        None => (text, None),
    }
}

/// Match one `[key:: value]` / `(key:: value)` field at the start of
/// `text`. Returns the key, the trimmed value, and the byte length
/// through the closer. The key is case-sensitive; spacing around `::` is
/// free. Values never nest: the first closer ends the field.
fn match_bracket_field(text: &str) -> Option<(String, String, usize)> {
    let (close, after_open) = if let Some(stripped) = text.strip_prefix('[') {
        (']', stripped)
    } else if let Some(stripped) = text.strip_prefix('(') {
        (')', stripped)
    } else {
        return None;
    };
    let end = after_open.find(close)?;
    let inner = &after_open[..end];
    let (raw_key, raw_value) = inner.split_once("::")?;
    let key = raw_key.trim();
    if key.is_empty()
        || !key.starts_with(|character: char| character.is_ascii_alphabetic())
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
        })
    {
        return None;
    }
    let full_len = 1 + end + close.len_utf8();
    Some((key.to_string(), raw_value.trim().to_string(), full_len))
}

/// Match a legacy bare `key:: value` field at the start of `text`.
/// Returns the key, the value, and the byte length consumed. The value
/// runs to the next ` #tag`, ` [`, ` (`, or the end of the segment.
fn match_bare_field(text: &str) -> Option<(String, String, usize)> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut key_end = None;
    for (index, character) in text.char_indices().skip(1) {
        if character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
        {
            continue;
        }
        key_end = Some(index);
        break;
    }
    let key_end = key_end.unwrap_or(text.len());
    let key = &text[..key_end];
    let gap_before = text[key_end..].len() - text[key_end..].trim_start().len();
    let after_key = text[key_end..].trim_start();
    // `::` must follow with whitespace or the end after it: `a::b` and
    // `12::30`-style tokens stay literal.
    let after_colons = after_key.strip_prefix("::")?;
    if !(after_colons.is_empty() || after_colons.starts_with([' ', '\t'])) {
        return None;
    }
    let gap_after = after_colons.len() - after_colons.trim_start().len();
    let mut value = after_colons.trim_start();
    for stop in [" #", " [", " ("] {
        if let Some(position) = value.find(stop) {
            value = value[..position].trim_end();
            break;
        }
    }
    let value = value.trim_end();
    if value.is_empty() {
        return None;
    }
    let consumed = key_end + gap_before + 2 + gap_after + value.len();
    Some((key.to_string(), value.to_string(), consumed))
}

/// True when `position` starts a tag: `#` followed by tag characters with
/// at least one letter, bounded on the left by a non-word character.
fn match_tag(text: &str, position: usize) -> Option<(String, usize)> {
    if !text[position..].starts_with('#') {
        return None;
    }
    if position > 0 {
        let prev = text[..position].chars().next_back();
        if prev.is_some_and(|character| {
            character.is_alphanumeric() || character == '_' || character == '/'
        }) {
            return None;
        }
    }
    let rest = &text[position + 1..];
    let len = rest
        .char_indices()
        .take_while(|(_, character)| {
            character.is_alphanumeric()
                || *character == '_'
                || *character == '-'
                || *character == '/'
        })
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    if len == 0 {
        return None;
    }
    let tag = &rest[..len];
    if !tag.chars().any(|character| character.is_alphabetic()) {
        return None;
    }
    Some((format!("#{tag}"), 1 + len))
}

impl<'a, L: SourceLoader> Projector<'a, L> {
    /// Parse one line's inline content. Code spans are literal except
    /// inline queries; every other construct resolves through the shared
    /// caches with document-order warnings.
    fn parse_inlines(
        &mut self,
        text: &str,
        line_no: Option<usize>,
    ) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut run_start = 0;
        let mut cursor = 0;
        while cursor < text.len() {
            if let Some((mut inlines, consumed)) =
                self.match_construct(text, cursor, line_no)
            {
                if cursor > run_start {
                    out.push(Inline::Text(text[run_start..cursor].to_string()));
                }
                out.append(&mut inlines);
                cursor += consumed;
                run_start = cursor;
            } else {
                cursor += text[cursor..]
                    .chars()
                    .next()
                    .map(|character| character.len_utf8())
                    .unwrap_or(1);
            }
        }
        if text.len() > run_start {
            out.push(Inline::Text(text[run_start..].to_string()));
        }
        merge_text_runs(&mut out);
        out
    }

    /// Try every inline construct at one cursor position. Returns the
    /// nodes and bytes consumed, or `None` to advance one character.
    fn match_construct(
        &mut self,
        text: &str,
        cursor: usize,
        line_no: Option<usize>,
    ) -> Option<(Vec<Inline>, usize)> {
        let rest = &text[cursor..];
        let prev = text[..cursor].chars().next_back();
        // Fenced-code-style protection never reaches this line parser, but
        // inline code does: a backtick run wins over every other construct.
        if rest.starts_with('`') && !is_escaped(text, cursor) {
            if let Some((content, consumed)) = match_code_span(text, cursor) {
                if is_inline_query(&content) {
                    return Some((
                        vec![Inline::Link {
                            label: vec![Inline::Text(
                                "Open live query".to_string(),
                            )],
                            target: LinkTarget::ObsidianUri(
                                self.host_file_uri.clone(),
                            ),
                        }],
                        consumed,
                    ));
                }
                return Some((vec![Inline::Code(content)], consumed));
            }
        }
        if rest.starts_with("![[") {
            if let Some(end) = rest.find("]]") {
                let inner = rest[3..end].to_string();
                if !inner.contains("[[") {
                    let inlines = self.project_inline_embed(&inner, line_no);
                    return Some((inlines, end + 2));
                }
            }
        }
        if rest.starts_with("[[") {
            if let Some(end) = rest.find("]]") {
                let inner = rest[2..end].to_string();
                let tail = rest[end + 2..].to_string();
                let (inlines, extra) =
                    self.project_content_link(&inner, &tail, line_no);
                return Some((inlines, end + 2 + extra));
            }
        }
        if rest.starts_with("![") && !rest.starts_with("![[") {
            if let Some(link) =
                self.match_markdown_link(text, cursor, line_no, true)
            {
                return Some(link);
            }
        }
        if rest.starts_with('[') {
            if let Some(link) =
                self.match_markdown_link(text, cursor, line_no, false)
            {
                return Some(link);
            }
            if let Some((key, value, len)) = match_bracket_field(rest) {
                if BOOKKEEPING_FIELDS.contains(&key.as_str()) {
                    return Some((Vec::new(), len));
                }
                return Some((
                    vec![Inline::Muted(vec![Inline::Text(format!(
                        "{key}: {value}"
                    ))])],
                    len,
                ));
            }
        }
        if rest.starts_with('(') {
            if let Some((key, value, len)) = match_bracket_field(rest) {
                if BOOKKEEPING_FIELDS.contains(&key.as_str()) {
                    return Some((Vec::new(), len));
                }
                return Some((
                    vec![Inline::Muted(vec![Inline::Text(format!(
                        "{key}: {value}"
                    ))])],
                    len,
                ));
            }
        }
        if rest.starts_with("~~") {
            if let Some((inner, consumed)) = match_delimited(rest, "~~") {
                let content = self.parse_inlines(&inner, line_no);
                return Some((vec![Inline::Strike(content)], consumed));
            }
        }
        if rest.starts_with("**") {
            if let Some((inner, consumed)) = match_delimited(rest, "**") {
                let content = self.parse_inlines(&inner, line_no);
                return Some((vec![Inline::Strong(content)], consumed));
            }
        }
        if rest.starts_with("__") {
            if let Some((inner, consumed)) =
                match_delimited_underscore(rest, 2, prev)
            {
                let content = self.parse_inlines(&inner, line_no);
                return Some((vec![Inline::Strong(content)], consumed));
            }
        }
        if rest.starts_with('*') {
            if let Some((inner, consumed)) = match_delimited(rest, "*") {
                let content = self.parse_inlines(&inner, line_no);
                return Some((vec![Inline::Emphasis(content)], consumed));
            }
        }
        if rest.starts_with('_') {
            if let Some((inner, consumed)) =
                match_delimited_underscore(rest, 1, prev)
            {
                let content = self.parse_inlines(&inner, line_no);
                return Some((vec![Inline::Emphasis(content)], consumed));
            }
        }
        if rest.starts_with('#') {
            if let Some((tag, len)) = match_tag(text, cursor) {
                if STRUCTURAL_TAGS.contains(&tag.as_str()) {
                    return Some((Vec::new(), len));
                }
                return Some((
                    vec![Inline::Muted(vec![Inline::Text(tag)])],
                    len,
                ));
            }
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let (url, len) = scan_autolink(rest);
            return Some((
                vec![Inline::Link {
                    label: vec![Inline::Text(url.clone())],
                    target: LinkTarget::Url(url),
                }],
                len,
            ));
        }
        if rest.starts_with("<http") {
            if let Some(end) = rest.find('>') {
                let url = rest[1..end].to_string();
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some((
                        vec![Inline::Link {
                            label: vec![Inline::Text(url.clone())],
                            target: LinkTarget::Url(url),
                        }],
                        end + 1,
                    ));
                }
            }
        }
        if rest.starts_with("~/") && is_word_boundary(prev) {
            let len = rest
                .find(|character: char| {
                    character.is_whitespace() || character == '<'
                })
                .unwrap_or(rest.len());
            let token = rest[..len].trim_end_matches([
                ',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\'',
            ]);
            if token.len() > 2 {
                return Some((
                    vec![Inline::Code(token.to_string())],
                    token.len(),
                ));
            }
        }
        // Legacy bare fields and long tokens start at word boundaries.
        if is_word_boundary(prev) {
            if let Some((key, value, len)) = match_bare_field(rest) {
                if BOOKKEEPING_FIELDS.contains(&key.as_str()) {
                    return Some((Vec::new(), len));
                }
                return Some((
                    vec![Inline::Muted(vec![Inline::Text(format!(
                        "{key}: {value}"
                    ))])],
                    len,
                ));
            }
            if let Some((token, len)) = match_long_token(rest) {
                return Some((vec![Inline::Code(token)], len));
            }
        }
        None
    }

    /// Parse `[label](target)` / `![alt](src)` at `cursor` (which points
    /// at `[`, or at `!` for images). Keep-link decorations become chips;
    /// local image sources resolve through the attachment index.
    fn match_markdown_link(
        &mut self,
        text: &str,
        cursor: usize,
        line_no: Option<usize>,
        image: bool,
    ) -> Option<(Vec<Inline>, usize)> {
        let start = if image { cursor + 1 } else { cursor };
        let (label_raw, after) = split_balanced(&text[start..], '[', ']')?;
        if !after.starts_with('(') {
            return None;
        }
        let (target_raw, tail) = split_balanced(&after, '(', ')')?;
        // Consumed ends where the tail begins.
        let consumed = text.len() - tail.len() - cursor;
        let (path, _) = split_image_target(&target_raw);
        let path = path.trim().to_string();
        if image {
            if path.is_empty() || is_url_target(&path) {
                let label = self.parse_inlines(&label_raw, line_no);
                if path.is_empty() {
                    return Some((label, consumed));
                }
                return Some((
                    vec![Inline::Link {
                        label,
                        target: LinkTarget::Url(path),
                    }],
                    consumed,
                ));
            }
            let alt = flatten_inlines(&self.parse_inlines(&label_raw, line_no));
            let attachment = parse_attachment_target(&path);
            let inlines = self.project_inline_image(
                &attachment.path,
                alt,
                attachment.width_px,
                line_no,
            );
            return Some((inlines, consumed));
        }
        let label = self.parse_inlines(&label_raw, line_no);
        if path.is_empty() {
            return Some((label, consumed));
        }
        if label_raw.trim() == "💡" && is_url_target(&path) {
            self.links_resolved += 1;
            return Some((
                vec![Inline::Chip(Chip {
                    content: vec![Inline::Text("Keep".to_string())],
                    kind: ChipKind::Normal,
                    target: Some(LinkTarget::Url(path)),
                })],
                consumed,
            ));
        }
        self.links_resolved += 1;
        Some((
            vec![Inline::Link {
                label,
                target: LinkTarget::Url(path),
            }],
            consumed,
        ))
    }
}

/// True when `prev` ends a word: start of text, whitespace, or an opening
/// punctuation mark. Field, path, and long-token rules all anchor here so
/// `a::b` and `C#tag` stay literal.
fn is_word_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(character) => {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | '['
                        | '{'
                        | '<'
                        | '"'
                        | '\''
                        | '>'
                        | ':'
                        | ';'
                        | ','
                        | '/'
                )
        }
    }
}

/// A non-space run of word-ish characters at least [`LONG_TOKEN_CHARS`]
/// long. Runs starting with markup characters are never long tokens.
fn match_long_token(text: &str) -> Option<(String, usize)> {
    if text.starts_with(['`', '!', '[', ']', '(', ')', '*', '_', '~', '#', '<'])
    {
        return None;
    }
    let len = text
        .find(|character: char| character.is_whitespace())
        .unwrap_or(text.len());
    let token = &text[..len];
    if token.chars().count() < LONG_TOKEN_CHARS {
        return None;
    }
    if !token.chars().all(|character| {
        character.is_alphanumeric()
            || matches!(
                character,
                '_' | '/' | '.' | '-' | '+' | '~' | ':' | '%' | '=' | '@'
            )
    }) {
        return None;
    }
    Some((token.to_string(), len))
}

/// Trim insignificant edge whitespace from parsed inline content:
/// dropped fields and tags otherwise leave trailing spaces behind.
fn trim_inline_edges(content: &mut Vec<Inline>) {
    if let Some(Inline::Text(first)) = content.first_mut() {
        *first = first.trim_start().to_string();
        if first.is_empty() {
            content.remove(0);
        }
    }
    if let Some(Inline::Text(last)) = content.last_mut() {
        *last = last.trim_end().to_string();
        if last.is_empty() {
            content.pop();
        }
    }
}

/// Merge adjacent text nodes left by construct splicing.
fn merge_text_runs(inlines: &mut Vec<Inline>) {
    let mut merged: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines.drain(..) {
        match inline {
            Inline::Text(next) => {
                if let Some(Inline::Text(last)) = merged.last_mut() {
                    last.push_str(&next);
                } else {
                    merged.push(Inline::Text(next));
                }
            }
            other => merged.push(other),
        }
    }
    *inlines = merged;
}

/// Match a backtick code span at `cursor`. Returns the raw content and
/// bytes consumed through the closer.
fn match_code_span(text: &str, cursor: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut open_end = cursor;
    while open_end < bytes.len() && bytes[open_end] == b'`' {
        open_end += 1;
    }
    let run = open_end - cursor;
    let mut search = open_end;
    while search < bytes.len() {
        if bytes[search] == b'`' && !is_escaped(text, search) {
            let mut close_end = search;
            while close_end < bytes.len() && bytes[close_end] == b'`' {
                close_end += 1;
            }
            if close_end - search == run {
                let mut content = text[open_end..search]
                    .replace('\n', " ")
                    .replace('\r', " ");
                if content.starts_with(' ')
                    && content.ends_with(' ')
                    && !content.trim().is_empty()
                {
                    content = content[1..content.len() - 1].to_string();
                }
                return Some((content, close_end - cursor));
            }
            search = close_end;
        } else {
            search += 1;
        }
    }
    None
}

/// True for an inline-query code span: `` `= ...` `` (Dataview inline
/// form). Ordinary code, including `a=b`, is never a query.
fn is_inline_query(content: &str) -> bool {
    match content.trim_start_matches([' ', '\t']).strip_prefix('=') {
        Some(rest) => rest.is_empty() || rest.starts_with([' ', '\t']),
        None => false,
    }
}

/// Match `~~`, `**`, or `*` at the start of `rest`. The opener needs
/// non-space after it; the closer needs non-space before it. Returns the
/// inner text and bytes consumed through the closer.
fn match_delimited(rest: &str, marker: &str) -> Option<(String, usize)> {
    let after_open = rest.strip_prefix(marker)?;
    if after_open.is_empty() || after_open.starts_with([' ', '\t', '\n']) {
        return None;
    }
    let mut search = 0;
    while let Some(position) = after_open[search..].find(marker) {
        let absolute = search + position;
        let before = after_open[..absolute].chars().next_back();
        let closer = &after_open[absolute + marker.len()..];
        let is_single_star = marker == "*" && {
            let after = closer.chars().next();
            let before_before = after_open[..absolute].chars().rev().nth(1);
            after == Some('*') || before_before == Some('*')
        };
        if before.is_some_and(|character| {
            !character.is_whitespace() && character != '\n'
        }) && !is_single_star
        {
            return Some((
                after_open[..absolute].to_string(),
                marker.len() + absolute + marker.len(),
            ));
        }
        search = absolute + 1;
    }
    None
}

/// Match `_`/`__` with intraword guards: the opener needs a word
/// boundary before it and non-space after; the closer needs non-space
/// before it and a word boundary after. `snake_case` stays literal.
fn match_delimited_underscore(
    rest: &str,
    width: usize,
    prev: Option<char>,
) -> Option<(String, usize)> {
    let marker = if width == 2 { "__" } else { "_" };
    let after_open = rest.strip_prefix(marker)?;
    if prev.is_some_and(|character| {
        character.is_alphanumeric() || character == '_'
    }) || after_open.is_empty()
        || after_open.starts_with([' ', '\t', '\n'])
    {
        return None;
    }
    let mut search = 0;
    while let Some(position) = after_open[search..].find(marker) {
        let absolute = search + position;
        let before = after_open[..absolute].chars().next_back();
        let after = after_open[absolute + marker.len()..].chars().next();
        let before_ok = before.is_some_and(|character| {
            !character.is_whitespace() && character != '\n'
        });
        let after_ok = after.is_none_or(|character| {
            !(character.is_alphanumeric() || character == '_')
        });
        if before_ok && after_ok {
            return Some((
                after_open[..absolute].to_string(),
                marker.len() + absolute + marker.len(),
            ));
        }
        search = absolute + 1;
    }
    None
}

/// Scan a bare `http(s)://` URL: to whitespace or `<`, trimming trailing
/// punctuation and closers that outnumber their openers.
fn scan_autolink(rest: &str) -> (String, usize) {
    let len = rest
        .find(|character: char| character.is_whitespace() || character == '<')
        .unwrap_or(rest.len());
    let mut url = rest[..len].to_string();
    while url.ends_with([',', '.', ';', ':', '!', '?', '"', '\'']) {
        url.pop();
    }
    while url.ends_with([')', ']']) {
        let opens = url.bytes().filter(|byte| *byte == b'(').count();
        let closes = url.bytes().filter(|byte| *byte == b')').count();
        let squares = url.ends_with(']') && !url.contains('[');
        if closes > opens || squares {
            url.pop();
        } else {
            break;
        }
    }
    (url.clone(), url.len())
}

/// Image extensions recognized before attachment resolution. A target
/// with one of these extensions takes the image flow even when the file
/// is missing (yielding a `.missing` placeholder); anything else first
/// tries the attachment index and falls back to note-embed resolution.
fn looks_like_image(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
    )
}

/// File stem of a vault-relative path for link labels.
fn note_stem(relative: &Path) -> String {
    relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Display route of a vault-relative path: `/`-joined, without `.md`.
fn note_route(relative: &Path) -> String {
    let route = relative.to_string_lossy().replace('\\', "/");
    route
        .strip_suffix(".md")
        .or_else(|| route.strip_suffix(".MD"))
        .unwrap_or(&route)
        .to_string()
}

/// Vault-relative path string with `/` separators for Obsidian URIs.
fn relative_uri_path(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

/// Fragment key shared with emitted anchors: normalized heading keys for
/// headings, raw ids for blocks.
fn fragment_key(fragment: &LinkFragment) -> String {
    match fragment {
        LinkFragment::Heading(heading) => normalize_heading_key_for(heading),
        LinkFragment::Block(id) => id.clone(),
    }
}

/// Readable fragment label for embed sources: heading text or `^id`.
fn fragment_label(fragment: &LinkFragment) -> String {
    match fragment {
        LinkFragment::Heading(heading) => heading.trim().to_string(),
        LinkFragment::Block(id) => format!("^{id}"),
    }
}

/// Describe a fragment for diagnostics: `#Heading` or `#^id`.
fn fragment_describe(fragment: &LinkFragment) -> String {
    match fragment {
        LinkFragment::Heading(heading) => format!("#{}", heading.trim()),
        LinkFragment::Block(id) => format!("#^{id}"),
    }
}

/// Truncate to a display width on character boundaries.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

impl<'a, L: SourceLoader> Projector<'a, L> {
    /// Project one `[[...]]` content link. `tail` is the text after the
    /// closing brackets (for paired `[h::]` consumption). Returns the
    /// nodes and extra bytes consumed past `]]`.
    fn project_content_link(
        &mut self,
        inner: &str,
        tail: &str,
        line_no: Option<usize>,
    ) -> (Vec<Inline>, usize) {
        let parsed = parse_link_target(inner);
        let is_quote = parsed.alias.as_deref() == Some("🔖")
            && matches!(parsed.fragment, Some(LinkFragment::Block(_)));
        // Recognize the quote link before its paired `[h:: ...]`
        // bookkeeping is stripped: consume the field here so the generic
        // field rule never sees it.
        let paired = if is_quote {
            consume_paired_h_field(tail)
        } else {
            0
        };
        let alias_inlines = parsed
            .alias
            .as_deref()
            .map(|alias| self.parse_inlines(alias, line_no));
        let (label, target, warning) =
            self.resolve_content_link(&parsed, line_no);
        let label = alias_inlines.unwrap_or(label);
        // Diagnostics never discard a usable target: an external link
        // with a missing fragment still carries its verbatim Obsidian
        // URI alongside the warning.
        if let Some((kind, message)) = warning {
            self.warn(kind, line_no, message);
        }
        let Some(target) = target else {
            self.links_unresolved += 1;
            return (label, paired);
        };
        self.links_resolved += 1;
        if is_quote {
            return (
                vec![Inline::Chip(Chip {
                    content: vec![Inline::Text("quote".to_string())],
                    kind: ChipKind::Normal,
                    target: Some(target),
                })],
                paired,
            );
        }
        (vec![Inline::Link { label, target }], paired)
    }

    /// Resolve one parsed content link to its display label and optional
    /// target. `None` means plain-label text with a diagnostic.
    fn resolve_content_link(
        &mut self,
        parsed: &ParsedLinkTarget,
        line_no: Option<usize>,
    ) -> (
        Vec<Inline>,
        Option<LinkTarget>,
        Option<(SnapshotWarningKind, String)>,
    ) {
        if parsed.note.is_empty() {
            let Some(fragment) = &parsed.fragment else {
                // Degenerate `[[]]`/`[[#]]`: literal text, honestly kept.
                return (vec![Inline::Text("[[#]]".to_string())], None, None);
            };
            return match self.host_index.lookup(fragment) {
                FragmentLookup::Found(span) => (
                    vec![Inline::Text(span.label)],
                    Some(LinkTarget::Internal(AnchorIdentity {
                        source: self.host.clone(),
                        kind: match fragment {
                            LinkFragment::Heading(_) => AnchorKind::Heading,
                            LinkFragment::Block(_) => AnchorKind::Block,
                        },
                        fragment: fragment_key(fragment),
                    })),
                    None,
                ),
                FragmentLookup::Missing => (
                    vec![Inline::Text(fragment_label(fragment))],
                    None,
                    Some((
                        SnapshotWarningKind::MissingFragment,
                        format!(
                            "no {} in this note",
                            fragment_describe(fragment)
                        ),
                    )),
                ),
                FragmentLookup::Ambiguous { count } => (
                    vec![Inline::Text(fragment_label(fragment))],
                    None,
                    Some((
                        SnapshotWarningKind::AmbiguousFragment,
                        format!(
                            "{count} headings named {} in this note",
                            fragment_describe(fragment)
                        ),
                    )),
                ),
            };
        }
        let resolved =
            match self.vault.resolve_note(&self.host_relative, parsed) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let kind = match &error {
                        NoteResolveError::Missing { .. } => {
                            SnapshotWarningKind::UnresolvedLink
                        }
                        NoteResolveError::Ambiguous { .. } => {
                            SnapshotWarningKind::AmbiguousLink
                        }
                    };
                    return (
                        vec![Inline::Text(parsed.note.clone())],
                        None,
                        Some((kind, error.to_string())),
                    );
                }
            };
        let stem = note_stem(&resolved.vault_relative_path);
        let is_self = resolved.canonical_path == self.host.canonical_path;
        let file = relative_uri_path(&resolved.vault_relative_path);
        match &parsed.fragment {
            None => {
                let label = vec![Inline::Text(stem)];
                let target = LinkTarget::ObsidianUri(obsidian_uri(
                    &self.vault_name,
                    &file,
                ));
                (label, Some(target), None)
            }
            Some(fragment) => {
                let uri = LinkTarget::ObsidianUri(obsidian_uri_with_fragment(
                    &self.vault_name,
                    &file,
                    Some(fragment),
                ));
                match fragment {
                    LinkFragment::Block(id) => {
                        if is_self {
                            if let Some(warning) =
                                self.check_host_fragment(fragment)
                            {
                                return (
                                    vec![Inline::Text(format!(
                                        "{stem} › ^{id}"
                                    ))],
                                    None,
                                    Some(warning),
                                );
                            }
                        }
                        let excerpt = self.block_excerpt(
                            &resolved.canonical_path,
                            id,
                            line_no,
                        );
                        let label = match excerpt {
                            Some(text) => {
                                vec![Inline::Text(format!("{stem} › {text}"))]
                            }
                            None => {
                                vec![Inline::Text(format!("{stem} › ^{id}"))]
                            }
                        };
                        if is_self {
                            return (
                                label,
                                Some(LinkTarget::Internal(AnchorIdentity {
                                    source: self.host.clone(),
                                    kind: AnchorKind::Block,
                                    fragment: id.clone(),
                                })),
                                None,
                            );
                        }
                        let warning =
                            self.check_external_fragment(&resolved, fragment);
                        (label, Some(uri), warning)
                    }
                    LinkFragment::Heading(_) => {
                        if is_self {
                            let label = match self.host_index.lookup(fragment) {
                                FragmentLookup::Found(span) => {
                                    vec![Inline::Text(span.label)]
                                }
                                _ => {
                                    vec![Inline::Text(fragment_label(fragment))]
                                }
                            };
                            let warning = self.check_host_fragment(fragment);
                            if warning.is_some() {
                                return (label, None, warning);
                            }
                            return (
                                label,
                                Some(LinkTarget::Internal(AnchorIdentity {
                                    source: self.host.clone(),
                                    kind: AnchorKind::Heading,
                                    fragment: fragment_key(fragment),
                                })),
                                warning,
                            );
                        }
                        let warning =
                            self.check_external_fragment(&resolved, fragment);
                        (vec![Inline::Text(stem)], Some(uri), warning)
                    }
                }
            }
        }
    }

    /// Validate a fragment against the host index for diagnostics.
    fn check_host_fragment(
        &self,
        fragment: &LinkFragment,
    ) -> Option<(SnapshotWarningKind, String)> {
        match self.host_index.lookup(fragment) {
            FragmentLookup::Found(_) => None,
            FragmentLookup::Missing => Some((
                SnapshotWarningKind::MissingFragment,
                format!("no {} in this note", fragment_describe(fragment)),
            )),
            FragmentLookup::Ambiguous { count } => Some((
                SnapshotWarningKind::AmbiguousFragment,
                format!(
                    "{count} {} named {} in this note",
                    match fragment {
                        LinkFragment::Heading(_) => "headings",
                        LinkFragment::Block(_) => "blocks",
                    },
                    fragment_describe(fragment)
                ),
            )),
        }
    }

    /// Validate a fragment against another note's cached bytes. Read
    /// failures and missing/ambiguous fragments warn; the Obsidian URI
    /// keeps its verbatim fragment either way.
    fn check_external_fragment(
        &mut self,
        resolved: &ResolvedNote,
        fragment: &LinkFragment,
    ) -> Option<(SnapshotWarningKind, String)> {
        let index = match self.fragment_index_for(&resolved.canonical_path) {
            Ok(index) => index?,
            Err(message) => {
                return Some((SnapshotWarningKind::ReadFailure, message));
            }
        };
        match index.lookup(fragment) {
            FragmentLookup::Found(_) => None,
            FragmentLookup::Missing => {
                let stem = note_stem(&resolved.vault_relative_path);
                Some((
                    SnapshotWarningKind::MissingFragment,
                    format!("no {} in '{stem}'", fragment_describe(fragment)),
                ))
            }
            FragmentLookup::Ambiguous { count } => {
                let stem = note_stem(&resolved.vault_relative_path);
                Some((
                    SnapshotWarningKind::AmbiguousFragment,
                    format!(
                        "{count} headings named {} in '{stem}'",
                        fragment_describe(fragment)
                    ),
                ))
            }
        }
    }

    /// Cached fragment index for another note's bytes: `Ok(None)` when
    /// the note is not valid UTF-8 text, `Err` with an actionable message
    /// on read failure.
    fn fragment_index_for(
        &mut self,
        canonical: &Path,
    ) -> Result<Option<FragmentIndex>, String> {
        if let Some(cached) = self.fragment_cache.get(canonical) {
            return Ok(cached.clone());
        }
        let bytes = match self.store.read_canonical(canonical) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.fragment_cache.insert(canonical.to_path_buf(), None);
                return Err(message);
            }
        };
        let index = String::from_utf8(bytes.to_vec())
            .ok()
            .map(|text| build_fragment_index(&text));
        self.fragment_cache
            .insert(canonical.to_path_buf(), index.clone());
        Ok(index)
    }

    /// Projection-safe excerpt for a block reference: the target line
    /// cleaned of markers, fields, tags, and block IDs, capped to
    /// [`EXCERPT_CHARS`]. Unknown-field meaning survives as `key: value`.
    fn block_excerpt(
        &mut self,
        canonical: &Path,
        id: &str,
        line_no: Option<usize>,
    ) -> Option<String> {
        let bytes = match self.store.read_canonical(canonical) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.warn(SnapshotWarningKind::ReadFailure, line_no, message);
                return None;
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let borrowed = text.lines().collect::<Vec<_>>();
        let fenced = fenced_lines(&borrowed, 0..borrowed.len());
        let comments = multiline_comment_lines(&borrowed);
        let frontmatter_end = strictly_closed_frontmatter_end(&borrowed)
            .map(|end| end + 1)
            .unwrap_or(0);
        for (index, line) in borrowed.iter().enumerate() {
            if index < frontmatter_end
                || fenced.contains(&index)
                || comments.contains(&index)
            {
                continue;
            }
            if trailing_block_id_in_line(line).as_deref() != Some(id) {
                continue;
            }
            let cleaned = clean_readable_text(line);
            if cleaned.is_empty() {
                return None;
            }
            return Some(truncate_chars(&cleaned, EXCERPT_CHARS));
        }
        None
    }

    /// Resolve and register one attachment. Failures warn once here, so
    /// block and inline flows share one diagnostic shape.
    fn resolve_attachment_image(
        &mut self,
        path: &str,
        width_px: Option<u32>,
        line_no: Option<usize>,
    ) -> Result<(SnapshotAsset, Option<u8>), AttachmentResolutionError> {
        let parsed = super::resolve::ParsedAttachmentTarget {
            path: path.to_string(),
            width_px,
        };
        let host_relative = self.host_relative.clone();
        match self.attachments.resolve(&host_relative, &parsed) {
            Ok(found) => {
                let width = width_px.and_then(width_hint_to_percent);
                let asset = self.register_asset(&found);
                Ok((asset, width))
            }
            Err(error) => {
                let message = error.to_string();
                let kind = match &error {
                    AttachmentResolutionError::Missing { .. } => {
                        SnapshotWarningKind::MissingImage
                    }
                    AttachmentResolutionError::Ambiguous { .. } => {
                        SnapshotWarningKind::AmbiguousImage
                    }
                };
                self.warn(kind, line_no, message);
                Err(error)
            }
        }
    }

    fn register_asset(&mut self, found: &ResolvedAttachment) -> SnapshotAsset {
        let asset = self.attachments.register_asset(found);
        if self.asset_paths.insert(asset.canonical_source_path.clone()) {
            self.assets.push(asset.clone());
        }
        asset
    }

    /// Block-level image: a real image asset or a `.missing` placeholder
    /// (resolution already warned).
    fn project_attachment_image(
        &mut self,
        path: &str,
        width_px: Option<u32>,
        alt: String,
        line_no: Option<usize>,
    ) -> Block {
        match self.resolve_attachment_image(path, width_px, line_no) {
            Ok((asset, width)) => Block::Image {
                asset_id: asset.id,
                alt,
                width_percent: width,
            },
            Err(_) => Block::MissingImage {
                label: vec![Inline::Text(format!("Missing image · {path}"))],
            },
        }
    }

    /// Inline image: resolved images degrade to links against the
    /// scratch-relative asset path; missing ones become muted text.
    fn project_inline_image(
        &mut self,
        path: &str,
        alt: String,
        width_px: Option<u32>,
        line_no: Option<usize>,
    ) -> Vec<Inline> {
        match self.resolve_attachment_image(path, width_px, line_no) {
            Ok((asset, width_percent)) => vec![Inline::Image {
                asset_id: asset.id,
                alt,
                width_percent,
            }],
            Err(_) => vec![Inline::Muted(vec![Inline::Text(format!(
                "Missing image · {path}"
            ))])],
        }
    }

    /// Block-level `![[...]]`: image, missing-image, or a structured
    /// note-embed request with empty contents for the transclusions phase.
    fn project_wikilink_embed(&mut self, inner: &str, line_no: usize) -> Block {
        let attach = parse_attachment_target(inner);
        // The probe resolve stays silent; the warning path below owns the
        // single diagnostic when the image flow is taken.
        let attachment_hit = self
            .attachments
            .resolve(&self.host_relative.clone(), &attach);
        let image_flow = looks_like_image(&attach.path)
            || matches!(&attachment_hit,
                Ok(found) if found.media_kind != super::view::AssetMediaKind::Other);
        if image_flow {
            let alt = note_stem(Path::new(&attach.path));
            let alt = if alt.is_empty() {
                attach.path.clone()
            } else {
                alt
            };
            return self.project_attachment_image(
                &attach.path,
                attach.width_px,
                alt,
                Some(line_no),
            );
        }
        let parsed = parse_link_target(inner);
        let fallback =
            parsed.alias.clone().unwrap_or_else(|| parsed.note.clone());
        let resolved =
            match self.vault.resolve_note(&self.host_relative, &parsed) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let kind = match &error {
                        NoteResolveError::Missing { .. } => {
                            SnapshotWarningKind::UnresolvedLink
                        }
                        NoteResolveError::Ambiguous { .. } => {
                            SnapshotWarningKind::AmbiguousLink
                        }
                    };
                    self.warn(kind, Some(line_no), error.to_string());
                    return Block::Paragraph(vec![Inline::Text(fallback)]);
                }
            };
        let route = note_route(&resolved.vault_relative_path);
        let label = match &parsed.fragment {
            Some(fragment) => {
                format!("{route} › {}", fragment_label(fragment))
            }
            None => route,
        };
        self.pending_embeds.push(PendingEmbed {
            label: label.clone(),
            canonical_path: resolved.canonical_path,
            vault_relative: resolved.vault_relative_path,
            fragment: parsed.fragment,
            origin_line: Some(line_no),
            same_note: resolved.same_note,
        });
        Block::Embed {
            source: label,
            blocks: Vec::new(),
        }
    }

    /// Inline `![[...]]` inside prose: images degrade to asset links,
    /// note embeds degrade to Obsidian links, failures to plain text.
    fn project_inline_embed(
        &mut self,
        inner: &str,
        line_no: Option<usize>,
    ) -> Vec<Inline> {
        let attach = parse_attachment_target(inner);
        let attachment_hit = self
            .attachments
            .resolve(&self.host_relative.clone(), &attach);
        let image_flow = looks_like_image(&attach.path)
            || matches!(&attachment_hit,
                Ok(found) if found.media_kind != super::view::AssetMediaKind::Other);
        if image_flow {
            let alt = note_stem(Path::new(&attach.path));
            let alt = if alt.is_empty() {
                attach.path.clone()
            } else {
                alt
            };
            return self.project_inline_image(
                &attach.path,
                alt,
                attach.width_px,
                line_no,
            );
        }
        let parsed = parse_link_target(inner);
        let fallback =
            parsed.alias.clone().unwrap_or_else(|| parsed.note.clone());
        let resolved =
            match self.vault.resolve_note(&self.host_relative, &parsed) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let kind = match &error {
                        NoteResolveError::Missing { .. } => {
                            SnapshotWarningKind::UnresolvedLink
                        }
                        NoteResolveError::Ambiguous { .. } => {
                            SnapshotWarningKind::AmbiguousLink
                        }
                    };
                    self.warn(kind, line_no, error.to_string());
                    self.links_unresolved += 1;
                    return vec![Inline::Text(fallback)];
                }
            };
        let route = note_route(&resolved.vault_relative_path);
        let label = match &parsed.fragment {
            Some(fragment) => {
                format!("{route} › {}", fragment_label(fragment))
            }
            None => route,
        };
        self.links_resolved += 1;
        self.pending_embeds.push(PendingEmbed {
            label: label.clone(),
            canonical_path: resolved.canonical_path,
            vault_relative: resolved.vault_relative_path,
            fragment: parsed.fragment,
            origin_line: line_no,
            same_note: resolved.same_note,
        });
        vec![Inline::EmbeddedBlock(Box::new(Block::Embed {
            source: label,
            blocks: Vec::new(),
        }))]
    }
}

/// Consume a paired `[h:: ...]` bookkeeping field after a quote link.
/// Returns bytes consumed (leading whitespace plus the field), or zero.
fn consume_paired_h_field(tail: &str) -> usize {
    let gap = tail.len() - tail.trim_start().len();
    let rest = tail.trim_start();
    if rest.starts_with('[') {
        if let Some((key, _, len)) = match_bracket_field(rest) {
            if key == "h" {
                return gap + len;
            }
        }
    }
    0
}

/// Clean one source line into projection-safe readable text for
/// block-reference excerpts: markers, bookkeeping fields, structural
/// tags, and block IDs are stripped; unknown fields survive as
/// `key: value`; links keep their alias-or-target label. Pure: no
/// resolution, no warnings.
fn clean_readable_text(line: &str) -> String {
    let mut text = line.trim().to_string();
    // One blockquote level, then heading markers, list markers, and the
    // checkbox symbol all read as prose in an excerpt.
    if let Some(stripped) = text.strip_prefix('>') {
        text = stripped.trim_start().to_string();
    }
    // Heading markers only: `#tag`-leading lines keep their tag for the
    // tag pass below.
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    if hashes > 0 && text[hashes..].starts_with([' ', '\t']) {
        text = text[hashes..].trim_start().to_string();
    }
    if let Some(marker) = parse_list_marker(&text) {
        text = marker.content.to_string();
    }
    // The list strip above leaves `[c] body` for task rows, including
    // the space symbol as `[ ]`.
    if text.starts_with('[') {
        if let Some(close) = text.find(']') {
            let symbol = text[1..close].trim();
            if symbol.chars().count() <= 1 {
                text = text[close + 1..].trim_start().to_string();
            }
        }
    }
    let (without_id, _) = strip_trailing_block_id(&text);
    text = without_id.to_string();
    text = replace_wikilinks_for_excerpt(&text);
    text = replace_markdown_links_for_excerpt(&text);
    text = strip_emphasis_markers(&text);
    text = replace_bracket_fields_for_excerpt(&text);
    text = replace_tags_for_excerpt(&text);
    text = replace_bare_fields_for_excerpt(&text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn replace_wikilinks_for_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let rest = &text[cursor..];
        let found = rest
            .find("![[")
            .map(|position| (position, 3))
            .or_else(|| rest.find("[[").map(|position| (position, 2)));
        let Some((open, skip)) = found else {
            out.push_str(rest);
            break;
        };
        if rest[open..].starts_with("[[[") {
            out.push_str(&rest[..open + 2]);
            cursor += open + 2;
            continue;
        }
        let Some(close) = rest[open + skip..].find("]]") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + skip..open + skip + close];
        let parsed = parse_link_target(inner);
        if let Some(alias) = parsed.alias {
            out.push_str(&alias);
        } else if parsed.note.is_empty() {
            match parsed.fragment {
                Some(fragment) => out.push_str(&fragment_label(&fragment)),
                None => out.push_str(inner),
            }
        } else {
            out.push_str(&parsed.note);
        }
        cursor += open + skip + close + 2;
    }
    out
}

fn replace_markdown_links_for_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let rest = &text[cursor..];
        let Some(open) = rest.find('[') else {
            out.push_str(rest);
            break;
        };
        let Some((label, after)) = split_balanced(&rest[open..], '[', ']')
        else {
            out.push_str(rest);
            break;
        };
        if after.starts_with('(') {
            if let Some((_, tail)) = split_balanced(&after, '(', ')') {
                out.push_str(&rest[..open]);
                out.push_str(&label);
                cursor = text.len() - tail.len();
                continue;
            }
        }
        out.push_str(&rest[..=open]);
        cursor += open + 1;
    }
    out
}

fn strip_emphasis_markers(text: &str) -> String {
    let without_strike = text.replace("~~", "");
    // Inline code keeps its content without backticks.
    let mut plain = String::new();
    let mut cursor = 0;
    while cursor < without_strike.len() {
        if without_strike[cursor..].starts_with('`') {
            let mut end = cursor + 1;
            while end < without_strike.len()
                && !without_strike[end..].starts_with('`')
            {
                end += 1;
            }
            plain.push_str(
                &without_strike[cursor + 1..end.min(without_strike.len())],
            );
            cursor = (end + 1).min(without_strike.len());
        } else {
            let next = without_strike[cursor..]
                .chars()
                .next()
                .map(|character| character.len_utf8())
                .unwrap_or(1);
            plain.push_str(&without_strike[cursor..cursor + next]);
            cursor += next;
        }
    }
    let mut out = plain;
    for marker in ["**", "__", "*", "_"] {
        out = out.replace(marker, "");
    }
    out
}

fn replace_bracket_fields_for_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let rest = &text[cursor..];
        if rest.starts_with('[') || rest.starts_with('(') {
            if let Some((key, value, len)) = match_bracket_field(rest) {
                if !BOOKKEEPING_FIELDS.contains(&key.as_str())
                    && !value.is_empty()
                {
                    out.push_str(&format!("{key}: {value}"));
                }
                out.push(' ');
                cursor += len;
                continue;
            }
        }
        let next = rest
            .chars()
            .next()
            .map(|character| character.len_utf8())
            .unwrap_or(1);
        out.push_str(&rest[..next]);
        cursor += next;
    }
    out
}

fn replace_tags_for_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        if text[cursor..].starts_with('#') {
            if let Some((tag, len)) = match_tag(text, cursor) {
                if !STRUCTURAL_TAGS.contains(&tag.as_str()) {
                    out.push_str(&tag);
                }
                cursor += len;
                continue;
            }
        }
        let next = text[cursor..]
            .chars()
            .next()
            .map(|character| character.len_utf8())
            .unwrap_or(1);
        out.push_str(&text[cursor..cursor + next]);
        cursor += next;
    }
    out
}

fn replace_bare_fields_for_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let prev = text[..cursor].chars().next_back();
        if is_word_boundary(prev) {
            if let Some((key, value, len)) = match_bare_field(&text[cursor..]) {
                if !BOOKKEEPING_FIELDS.contains(&key.as_str()) {
                    out.push_str(&format!("{key}: {value} "));
                }
                cursor += len;
                continue;
            }
        }
        let next = text[cursor..]
            .chars()
            .next()
            .map(|character| character.len_utf8())
            .unwrap_or(1);
        out.push_str(&text[cursor..cursor + next]);
        cursor += next;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use super::super::classify::{source_identity, FilesystemSourceLoader};
    use super::super::resolve::percent_encode;
    use super::super::view::{
        SnapshotMasthead, SnapshotSection, SnapshotSummary, SnapshotView,
    };
    use super::*;
    use crate::native::highlights_ref::note_snapshot::serialize_projected_markdown;
    use crate::native::task_status_hooks::{TaskStatusType, TasksSettings};

    /// Vault Tasks settings mirroring the corpus: `*` Next, `?` Blocked,
    /// `/` In Progress, `-` Canceled.
    fn fake_settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
                ('*', TaskStatusType::OnHold),
                ('?', TaskStatusType::OnHold),
                ('-', TaskStatusType::Cancelled),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn test_vault(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp vault");
        for (relative, contents) in files {
            let path = directory.path().join(relative);
            fs::create_dir_all(path.parent().expect("parent"))
                .expect("create parent");
            fs::write(&path, contents).expect("write file");
        }
        directory
    }

    fn project_host(
        files: &[(&str, &str)],
        host: &str,
    ) -> (tempfile::TempDir, SyntaxProjection) {
        let directory = test_vault(files);
        let root = directory.path().canonicalize().expect("canonical root");
        let mut store = SourceStore::with_loader(&root, FilesystemSourceLoader)
            .expect("open store");
        let vault_root = store.vault_root().to_path_buf();
        let canonical = root.join(host).canonicalize().expect("host exists");
        let identity =
            source_identity(&vault_root, &canonical).expect("identity");
        let mut attachments = AttachmentIndex::new(vault_root);
        let projection = project_obsidian_syntax(
            &mut store,
            &mut attachments,
            &identity,
            &fake_settings(),
        )
        .expect("project host");
        (directory, projection)
    }

    fn host_only(contents: &str) -> (tempfile::TempDir, SyntaxProjection) {
        project_host(&[("Host.md", contents)], "Host.md")
    }

    /// One-line debug shape per block for compact structural assertions.
    fn block_shape(block: &Block) -> String {
        match block {
            Block::Paragraph(inlines) => {
                format!("P:{}", flatten_inlines(inlines))
            }
            Block::Heading(heading) => format!(
                "H:{}:{}",
                heading.level.marker(),
                flatten_inlines(&heading.content)
            ),
            Block::Task(task) => format!(
                "T:{}:{}",
                task.status.as_str(),
                flatten_inlines(&task.content)
            ),
            Block::List { kind, items } => {
                let marker = match kind {
                    ListKind::Unordered => "-",
                    ListKind::Ordered { start } => {
                        return format!(
                            "L:ordered@{start}:{}items",
                            items.len()
                        );
                    }
                };
                format!("L:{marker}:{}items", items.len())
            }
            Block::Code { language, text } => {
                format!("C:{language:?}:{}bytes", text.len())
            }
            Block::LabeledRow { .. } => "labeled".to_string(),
            Block::LogRow { .. } => "log".to_string(),
            Block::Image {
                asset_id,
                width_percent,
                ..
            } => format!("I:{asset_id}:{width_percent:?}"),
            Block::MissingImage { label } => {
                format!("MI:{}", flatten_inlines(label))
            }
            Block::Embed { source, blocks } => {
                format!("E:{source}:{}blocks", blocks.len())
            }
            Block::LiveQuery { .. } => "Q".to_string(),
            Block::Callout { kind, title, .. } => {
                format!("CO:{kind}:{}", flatten_inlines(title))
            }
            Block::Compact { .. } => "compact".to_string(),
            Block::EmptyState(content) => {
                format!("empty:{}", flatten_inlines(content))
            }
        }
    }

    fn shapes(blocks: &[Block]) -> Vec<String> {
        blocks.iter().map(block_shape).collect()
    }

    fn warnings_of(
        projection: &SyntaxProjection,
        kind: SnapshotWarningKind,
    ) -> Vec<String> {
        projection
            .warnings
            .iter()
            .filter(|warning| warning.kind == kind)
            .map(|warning| warning.message.clone())
            .collect()
    }

    #[test]
    fn status_symbols_map_through_tasks_settings() {
        let settings = fake_settings();
        let cases = [
            (' ', SnapshotTaskStatus::Open),
            ('x', SnapshotTaskStatus::Done),
            ('X', SnapshotTaskStatus::Done),
            ('*', SnapshotTaskStatus::Next),
            ('/', SnapshotTaskStatus::Wip),
            ('?', SnapshotTaskStatus::Blocked),
            ('-', SnapshotTaskStatus::Canceled),
        ];
        for (symbol, expected) in cases {
            assert_eq!(
                status_for_symbol(symbol, &settings),
                Some(expected),
                "symbol {symbol}"
            );
        }
        // Unconfigured symbols are unknown, never silently derived.
        assert_eq!(status_for_symbol('!', &settings), None);
        // Settings fall back for project-specific symbols.
        let mut custom = fake_settings();
        custom.status_types.insert('~', TaskStatusType::Done);
        assert_eq!(
            status_for_symbol('~', &custom),
            Some(SnapshotTaskStatus::Done)
        );
    }

    #[test]
    fn frontmatter_is_dropped_but_retained() {
        let (_dir, projection) = host_only(
            "---\ntype: '[[project]]'\ncreated: 2026-10-15\n---\n\nBody text.\n",
        );
        let front = projection.parsed.frontmatter.expect("frontmatter");
        assert!(front.raw_yaml.contains("type:"));
        assert_eq!(
            front.metadata["created"],
            serde_yaml::Value::from("2026-10-15")
        );
        assert_eq!(front.extent.first_line, 1);
        assert_eq!(shapes(&projection.blocks), vec!["P:Body text."]);
    }

    #[test]
    fn comments_drop_outside_code_only() {
        let (_dir, projection) = host_only(
            "kept %% inline dropped %% here\n\
             ```\n%% literal in fence %%\n[[also literal]]\n```\n\
             <!-- html dropped --> tail\n\
             %%\nmultiline dropped\n%%\n\
             after\n",
        );
        let shape = shapes(&projection.blocks).join("\n");
        assert!(shape.contains("kept  here"), "{shape}");
        assert!(shape.contains("tail"), "{shape}");
        assert!(shape.contains("after"), "{shape}");
        assert!(!shape.contains("dropped"), "{shape}");
        assert!(!shape.contains("multiline"), "{shape}");
        let code = projection
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Code { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("code block");
        assert!(code.contains("%% literal in fence %%"));
        assert!(code.contains("[[also literal]]"));
    }

    #[test]
    fn comment_scanner_preserves_visible_text_around_multiline_comments() {
        let (_dir, projection) = host_only(
            "# Visible %%private%% title\n\
             Before <!-- hidden start\n\
             # Hidden heading\n\
             --> after\n\
             ```text\n%%\n<!--\n```\n",
        );
        let shape = shapes(&projection.blocks).join("\n");
        assert!(shape.contains("H:#:Visible  title"), "{shape}");
        assert!(shape.contains("Before"), "{shape}");
        assert!(shape.contains("after"), "{shape}");
        assert!(!shape.contains("private"), "{shape}");
        assert!(!shape.contains("Hidden heading"), "{shape}");
        let code = projection
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Code { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .expect("fenced code");
        assert_eq!(code, "%%\n<!--\n");
    }

    #[test]
    fn inline_code_protects_apparent_syntax() {
        let (_dir, projection) =
            host_only("Use `[[not a link]]` and `#notatag` here.\n");
        let Block::Paragraph(inlines) = &projection.blocks[0] else {
            panic!("paragraph: {:?}", projection.blocks);
        };
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Code(code) if code == "[[not a link]]"
        )));
        assert!(
            !projection
                .warnings
                .iter()
                .any(|warning| warning.kind
                    == SnapshotWarningKind::UnresolvedLink)
        );
    }

    #[test]
    fn nondefault_fence_lengths_stay_literal() {
        let (_dir, projection) =
            host_only("````\n[[link]] [f:: x] #tag ^block\n````\n");
        assert_eq!(projection.blocks.len(), 1);
        let Block::Code { text, language } = &projection.blocks[0] else {
            panic!("code: {:?}", projection.blocks);
        };
        assert_eq!(language, &None);
        assert!(text.contains("[[link]] [f:: x] #tag ^block"));
        assert!(projection.warnings.is_empty());
    }

    #[test]
    fn headings_shift_collide_and_offer_a_title() {
        let (_dir, projection) = host_only(
            "# Real Title\n\n## Dup\n\ntext\n## Dup\n\n### Deep ^h-id\n",
        );
        assert_eq!(
            shapes(&projection.blocks),
            vec![
                "H:#:Real Title",
                "H:##:Dup",
                "P:text",
                "H:##:Dup",
                "H:###:Deep",
            ]
        );
        let ids = projection
            .anchors
            .iter()
            .map(|anchor| anchor.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["real-title", "dup", "dup-2", "deep", "t-h-id"]);
        let title = projection.title_candidate.expect("title");
        assert_eq!(title.display_text, "Real Title");
        assert_eq!(title.anchor_id, "real-title");
    }

    #[test]
    fn shallowest_heading_becomes_h1() {
        let (_dir, projection) =
            host_only("## Top\n\nbody\n### Sub\n\n#### Deep\n");
        assert_eq!(
            shapes(&projection.blocks),
            vec!["H:#:Top", "P:body", "H:##:Sub", "H:###:Deep"]
        );
        // No original H1, so no title candidate to consume.
        assert!(projection.title_candidate.is_none());
    }

    #[test]
    fn heading_without_a_blank_line_before_it() {
        let (_dir, projection) = host_only("body text\n## Glued\nmore\n");
        assert_eq!(
            shapes(&projection.blocks),
            vec!["P:body text", "H:#:Glued", "P:more"]
        );
    }

    #[test]
    fn every_checkbox_status_without_a_filter() {
        let (_dir, projection) = host_only(
            "- [ ] open, no filter tag\n\
             - [x] done #task\n\
             - [X] done upper #task\n\
             - [*] next #task\n\
             - [/] wip #task\n\
             - [?] blocked #task\n\
             - [-] canceled #task\n\
             - [!] odd #task\n",
        );
        assert_eq!(
            shapes(&projection.blocks),
            vec![
                "T:open:open, no filter tag",
                "T:done:done",
                "T:done:done upper",
                "T:next:next",
                "T:wip:wip",
                "T:blocked:blocked",
                "T:canceled:canceled",
                "T:open:odd",
            ]
        );
        let unknown =
            warnings_of(&projection, SnapshotWarningKind::UnknownTaskStatus);
        assert_eq!(unknown.len(), 1);
        assert!(unknown[0].contains("[!]"), "{}", unknown[0]);
    }

    #[test]
    fn nested_children_survive_mixed_tabs_and_spaces() {
        let (_dir, projection) = host_only(
            "- [ ] parent\n  - [ ] two-space child\n\t- [ ] tab grandchild\n- [ ] sibling\n",
        );
        assert_eq!(projection.blocks.len(), 2);
        let Block::Task(parent) = &projection.blocks[0] else {
            panic!("parent: {:?}", projection.blocks);
        };
        assert_eq!(parent.children.len(), 1);
        let Block::Task(child) = &parent.children[0] else {
            panic!("child: {:?}", parent.children);
        };
        assert_eq!(flatten_inlines(&child.content), "two-space child");
        assert_eq!(child.children.len(), 1);
        // Raw source rows survive byte-for-byte for the Bob layer.
        let bodies = projection
            .parsed
            .task_lines
            .iter()
            .map(|line| line.body.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            bodies,
            vec!["parent", "two-space child", "tab grandchild", "sibling",]
        );
        assert_eq!(projection.parsed.task_lines[2].indentation.columns, 4);
    }

    #[test]
    fn task_source_records_keep_comments_fields_indentation_and_callout_offsets(
    ) {
        let input = "    - [ ] Keep %%private%% [fresh:: 2026-10-16] ^item\n\
                     \t- [ ] Tab [custom:: retained]\n\
                     > [!note]\n\
                     > - [ ] Nested %%secret%% [custom:: callout] ^nested\n";
        let (_dir, projection) = host_only(input);
        assert_eq!(projection.parsed.task_lines.len(), 3);
        let expected = [
            (
                "Keep %%private%% [fresh:: 2026-10-16] ^item",
                "    ",
                1,
                "    - [ ] Keep %%private%% [fresh:: 2026-10-16] ^item",
            ),
            (
                "Tab [custom:: retained]",
                "\t",
                2,
                "\t- [ ] Tab [custom:: retained]",
            ),
            (
                "Nested %%secret%% [custom:: callout] ^nested",
                "",
                4,
                "> - [ ] Nested %%secret%% [custom:: callout] ^nested",
            ),
        ];
        for (task, (body, indent, line, raw_line)) in
            projection.parsed.task_lines.iter().zip(expected)
        {
            assert_eq!(task.body, body);
            assert_eq!(task.indentation.prefix, indent);
            assert_eq!(task.extent.first_line, line);
            assert_eq!(task.raw_line, raw_line);
            assert_eq!(
                &projection.parsed.raw_bytes
                    [task.extent.byte_start..task.extent.byte_end],
                raw_line.as_bytes()
            );
        }
    }

    #[test]
    fn remote_and_malformed_image_lines_always_make_bounded_progress() {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (_dir, projection) = host_only(
                "![remote](https://example.test/image.png)\n\
                 \n\
                 Adjacent ![remote](https://example.test/image.png) prose\n\
                 \n\
                 ![broken](https://example.test/image.png\n",
            );
            let blocks = projection.blocks;
            let _ = sender.send(blocks);
        });
        let blocks = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect(
                "snapshot parser did not advance through image-shaped lines",
            );
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        let remote_links = blocks[..2]
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) => inlines.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .filter(|inline| {
                matches!(
                    inline,
                    Inline::Link { target: LinkTarget::Url(url), .. }
                        if url == "https://example.test/image.png"
                )
            })
            .count();
        assert_eq!(remote_links, 2);
        let Block::Paragraph(malformed) = &blocks[2] else {
            panic!("malformed image: {blocks:?}");
        };
        assert!(flatten_inlines(malformed).contains("![broken]"));
    }

    #[test]
    fn bookkeeping_drops_while_unknown_fields_stay_muted() {
        let (_dir, projection) = host_only(
            "- [ ] work [fresh:: 2026-10-01] [created:: 2026-10-02] [id:: a] \
             [dependsOn:: [[x]]] [keeps:: 1] [unblocked:: 2026-10-03] \
             [h:: y] [gkeep:: z] [priority:: high] due:: tomorrow #task #hide #prj_is_blocked #keepme\n",
        );
        let Block::Task(task) = &projection.blocks[0] else {
            panic!("task: {:?}", projection.blocks);
        };
        let flat = flatten_inlines(&task.content);
        for dropped in [
            "fresh",
            "created",
            "[id",
            "dependsOn",
            "keeps",
            "unblocked",
            "[h::",
            "gkeep",
            "#task",
            "#hide",
            "#prj_is_blocked",
        ] {
            assert!(!flat.contains(dropped), "{flat}");
        }
        assert!(flat.contains("priority: high"), "{flat}");
        assert!(flat.contains("due: tomorrow"), "{flat}");
        assert!(flat.contains("#keepme"), "{flat}");
        // The raw body keeps everything for Bob-layer chip creation.
        let raw = &projection.parsed.task_lines[0];
        assert!(raw.body.contains("[priority:: high]"));
        assert!(raw.body.contains("[fresh:: 2026-10-01]"));
        assert!(raw.block_id.is_none());
    }

    #[test]
    fn trailing_block_ids_become_task_anchors() {
        let (_dir, projection) =
            host_only("- [ ] anchored #task ^my-id\n\nplain ^para\n");
        let Block::Task(task) = &projection.blocks[0] else {
            panic!("task: {:?}", projection.blocks);
        };
        let anchor = task.anchor.as_ref().expect("anchor");
        assert_eq!(anchor.id, "t-my-id");
        assert_eq!(anchor.identity.fragment, "my-id");
        assert_eq!(flatten_inlines(&task.content), "anchored");
        assert_eq!(
            &projection.parsed.task_lines[0].block_id,
            &Some("my-id".to_string())
        );
        let ids = projection
            .anchors
            .iter()
            .map(|anchor| anchor.id.clone())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"t-my-id".to_string()), "{ids:?}");
        assert!(ids.contains(&"t-para".to_string()), "{ids:?}");
    }

    #[test]
    fn keep_and_quote_decorations_become_chips() {
        let (_dir, projection) = project_host(
            &[
                (
                    "Host.md",
                    "- [ ] keep it [💡](https://keep.example.test/x \"Open in Google Keep\") #task\n\
                     - [ ] read it [[Other#^h-abc|🔖]] [h:: abc] #task\n\
                     - [ ] stray [h:: orphan] #task\n",
                ),
                ("Other.md", "Note.\n\nQuoted line here. ^h-abc\n"),
            ],
            "Host.md",
        );
        assert_eq!(projection.blocks.len(), 3);
        let Block::Task(keep) = &projection.blocks[0] else {
            panic!("keep: {:?}", projection.blocks);
        };
        let chips = keep
            .content
            .iter()
            .filter_map(|inline| match inline {
                Inline::Chip(chip) => Some(chip),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(chips.len(), 1);
        assert_eq!(flatten_inlines(&chips[0].content), "Keep");
        assert!(matches!(chips[0].target, Some(LinkTarget::Url(_))));
        let Block::Task(quote) = &projection.blocks[1] else {
            panic!("quote: {:?}", projection.blocks);
        };
        let flat = flatten_inlines(&quote.content);
        assert!(flat.contains("quote"), "{flat}");
        assert!(!flat.contains("h::"), "{flat}");
        let Block::Task(stray) = &projection.blocks[2] else {
            panic!("stray: {:?}", projection.blocks);
        };
        let flat = flatten_inlines(&stray.content);
        assert!(!flat.contains("h::"), "{flat}");
        assert!(flat.contains("stray"), "{flat}");
    }

    fn link_targets(blocks: &[Block]) -> Vec<String> {
        let mut targets = Vec::new();
        fn walk(inlines: &[Inline], targets: &mut Vec<String>) {
            for inline in inlines {
                match inline {
                    Inline::Link { label, target } => {
                        let destination = match target {
                            LinkTarget::Url(url) => format!("url:{url}"),
                            LinkTarget::ObsidianUri(uri) => {
                                format!("obs:{uri}")
                            }
                            LinkTarget::Internal(identity) => {
                                format!("int:{}", identity.fragment)
                            }
                            LinkTarget::ScopedInternal {
                                identity,
                                presentation_id,
                            } => format!(
                                "scoped:{}=>{}",
                                identity.fragment, presentation_id
                            ),
                        };
                        targets.push(format!(
                            "{}=>{destination}",
                            flatten_inlines(label)
                        ));
                        walk(label, targets);
                    }
                    Inline::Emphasis(content)
                    | Inline::Strong(content)
                    | Inline::Strike(content)
                    | Inline::Muted(content) => walk(content, targets),
                    Inline::Chip(chip) => walk(&chip.content, targets),
                    _ => {}
                }
            }
        }
        fn blocks_walk(blocks: &[Block], targets: &mut Vec<String>) {
            for block in blocks {
                match block {
                    Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                        walk(inlines, targets);
                    }
                    Block::Heading(heading) => walk(&heading.content, targets),
                    Block::List { items, .. } => {
                        for item in items {
                            walk(&item.content, targets);
                            blocks_walk(&item.children, targets);
                        }
                    }
                    Block::Task(task) => {
                        walk(&task.content, targets);
                        blocks_walk(&task.children, targets);
                    }
                    Block::Callout { title, blocks, .. } => {
                        walk(title, targets);
                        blocks_walk(blocks, targets);
                    }
                    Block::Embed { blocks, .. }
                    | Block::LiveQuery { blocks }
                    | Block::Compact { blocks } => {
                        blocks_walk(blocks, targets);
                    }
                    _ => {}
                }
            }
        }
        blocks_walk(blocks, &mut targets);
        targets
    }

    #[test]
    fn wikilinks_resolve_alias_fragments_and_failures() {
        let (_dir, projection) = project_host(
            &[
                (
                    "Host.md",
                    "See [[Other|the other note]] and [[Other#Details]] \
                     and [[Other#^t1]] and [[Missing Note]] and [[#Host Title]].\n\
                     \n\
                     # Host Title\n",
                ),
                (
                    "Other.md",
                    "# Other Note\n\n## Details\n\nBody.\n\n- [ ] Other task [custom:: kept] #task ^t1\n",
                ),
            ],
            "Host.md",
        );
        let targets = link_targets(&projection.blocks);
        assert!(
            targets
                .iter()
                .any(|target| target.starts_with("the other note=>obs:")),
            "{targets:?}"
        );
        // The temp vault name is random; pin the stable URI shape only.
        // External heading links keep the note stem as their label while
        // the URI retains the verbatim fragment for Obsidian.
        assert!(
            targets.iter().any(|target| {
                target.starts_with("Other=>obs:obsidian://open?vault=")
                    && target.contains("&file=Other.md")
            }),
            "{targets:?}"
        );
        // Block refs without an alias read `Note › cleaned task text`
        // with unknown-field meaning preserved.
        assert!(
            targets.iter().any(|target| target
                .starts_with("Other › Other task custom: kept=>obs:")),
            "{targets:?}"
        );
        // Same-note heading links stay internal.
        assert!(
            targets
                .iter()
                .any(|target| target == "Host Title=>int:host title"),
            "{targets:?}"
        );
        // The missing note is plain text with a typed warning.
        assert!(
            !targets
                .iter()
                .any(|target| target.starts_with("Missing Note=>")),
            "{targets:?}"
        );
        let missing =
            warnings_of(&projection, SnapshotWarningKind::UnresolvedLink);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("Missing Note"), "{}", missing[0]);
        assert!(
            projection.links_unresolved == 1,
            "{}",
            projection.links_unresolved
        );
        assert!(
            projection.links_resolved >= 4,
            "{}",
            projection.links_resolved
        );
    }

    #[test]
    fn ambiguous_notes_warn_without_choosing() {
        let (_dir, projection) = project_host(
            &[
                ("Host.md", "See [[Twin]] here.\n"),
                ("a/Twin.md", "# Twin A\n"),
                ("b/Twin.md", "# Twin B\n"),
            ],
            "Host.md",
        );
        let ambiguous =
            warnings_of(&projection, SnapshotWarningKind::AmbiguousLink);
        assert_eq!(ambiguous.len(), 1);
        assert!(ambiguous[0].contains("Twin"), "{}", ambiguous[0]);
        let flat = shapes(&projection.blocks).join("\n");
        assert!(flat.contains("Twin"), "{flat}");
        assert!(!flat.contains("obsidian://"), "{flat}");
    }

    #[test]
    fn missing_fragments_warn_while_keeping_the_uri() {
        let (_dir, projection) = project_host(
            &[
                ("Host.md", "See [[Other#Absent]] and [[#Absent]].\n"),
                ("Other.md", "# Other\n"),
            ],
            "Host.md",
        );
        let missing =
            warnings_of(&projection, SnapshotWarningKind::MissingFragment);
        assert_eq!(missing.len(), 2, "{missing:?}");
        // The external link keeps its verbatim fragment for Obsidian.
        let targets = link_targets(&projection.blocks);
        assert!(
            targets
                .iter()
                .any(|target| target.starts_with("Other=>obs:")),
            "{targets:?}"
        );
    }

    #[test]
    fn excerpt_cleaning_preserves_unknown_field_meaning() {
        assert_eq!(
            clean_readable_text(
                "- [?] blocked [custom:: value] #task ^task-id"
            ),
            "blocked custom: value"
        );
        assert_eq!(clean_readable_text("## Requirements ^req"), "Requirements");
        assert_eq!(
            clean_readable_text(
                "See [[Note|Alias]] and **bold** `code` #keep #task"
            ),
            "See Alias and bold code #keep"
        );
    }

    #[test]
    fn images_register_widths_and_missing_placeholders() {
        let (_dir, projection) = project_host(
            &[
                (
                    "Host.md",
                    "![[img/a.png|400]]\n\n![[img/gone.png]]\n\n![alt text](img/a.png)\n",
                ),
                ("img/a.png", "fake-png-bytes"),
            ],
            "Host.md",
        );
        assert_eq!(projection.assets.len(), 1);
        assert_eq!(projection.assets[0].width_percent, Some(62));
        assert_eq!(
            projection.assets[0].relative_path.to_string_lossy(),
            "assets/1.png"
        );
        let shape_list = shapes(&projection.blocks);
        assert_eq!(shape_list.len(), 3);
        assert!(shape_list[0].starts_with("I:a1:Some(62)"), "{shape_list:?}");
        assert!(
            shape_list[1].starts_with("MI:Missing image · img/gone.png"),
            "{shape_list:?}"
        );
        let missing =
            warnings_of(&projection, SnapshotWarningKind::MissingImage);
        assert_eq!(missing.len(), 1);
        // The Markdown-image twin reuses the same registered asset.
        assert!(shape_list[2].starts_with("I:a1:"), "{shape_list:?}");
    }

    #[test]
    fn note_embeds_stay_structured_requests() {
        let (_dir, projection) = project_host(
            &[
                (
                    "Host.md",
                    "![[Other]]\n\n![[Other#Details]]\n\n![[Other#^t1]]\n\n![[Gone]]\n",
                ),
                (
                    "Other.md",
                    "# Other\n\n## Details\n\nBody.\n\n- [ ] Other task #task ^t1\n",
                ),
            ],
            "Host.md",
        );
        assert_eq!(projection.pending_embeds.len(), 3);
        assert_eq!(projection.pending_embeds[0].label, "Other");
        assert_eq!(projection.pending_embeds[1].label, "Other › Details");
        assert_eq!(projection.pending_embeds[2].label, "Other › ^t1");
        assert_eq!(
            projection.pending_embeds[2].fragment,
            Some(LinkFragment::Block("t1".to_string()))
        );
        // Placeholders keep document order; the missing note degrades to
        // readable text with a warning.
        assert_eq!(
            shapes(&projection.blocks),
            vec![
                "E:Other:0blocks",
                "E:Other › Details:0blocks",
                "E:Other › ^t1:0blocks",
                "P:Gone",
            ]
        );
        let missing =
            warnings_of(&projection, SnapshotWarningKind::UnresolvedLink);
        assert_eq!(missing.len(), 1);
    }

    #[test]
    fn callouts_keep_type_title_and_body() {
        let (_dir, projection) = host_only(
            "> [!warning] Watch out\n> second line\n\n> plain quote\n",
        );
        assert_eq!(projection.blocks.len(), 2);
        let Block::Callout {
            kind,
            title,
            blocks,
        } = &projection.blocks[0]
        else {
            panic!("callout: {:?}", projection.blocks);
        };
        assert_eq!(kind, "warning");
        assert_eq!(flatten_inlines(title), "Watch out");
        assert_eq!(shapes(blocks), vec!["P:second line"]);
        let Block::Callout { kind, blocks, .. } = &projection.blocks[1] else {
            panic!("plain: {:?}", projection.blocks);
        };
        assert_eq!(kind, "note");
        assert_eq!(shapes(blocks), vec!["P:plain quote"]);
    }

    #[test]
    fn queries_become_linked_placeholders_without_executing() {
        let (_dir, projection) = host_only(
            "```dataview\nTABLE x FROM y\n```\n\n```tasks\nnot done\n```\n\nSee `= 2 + 2` and `a=b`.\n",
        );
        assert_eq!(projection.blocks.len(), 3);
        assert!(matches!(projection.blocks[0], Block::LiveQuery { .. }));
        assert!(matches!(projection.blocks[1], Block::LiveQuery { .. }));
        let Block::LiveQuery { blocks } = &projection.blocks[0] else {
            unreachable!();
        };
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("livequery body: {blocks:?}");
        };
        assert!(matches!(
            &inlines[0],
            Inline::Link {
                target: LinkTarget::ObsidianUri(_),
                ..
            }
        ));
        let Block::Paragraph(inlines) = &projection.blocks[2] else {
            panic!("para: {:?}", projection.blocks);
        };
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link { label, .. }
            if flatten_inlines(label) == "Open live query"
        )));
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Code(code) if code == "a=b"
        )));
    }

    #[test]
    fn strike_plus_wikilink_keeps_links_liftable() {
        let (_dir, projection) = project_host(
            &[
                ("Host.md", "~~struck [[Other|visible]] tail~~\n"),
                ("Other.md", "# Other\n"),
            ],
            "Host.md",
        );
        let Block::Paragraph(inlines) = &projection.blocks[0] else {
            panic!("para: {:?}", projection.blocks);
        };
        assert_eq!(inlines.len(), 1);
        let Inline::Strike(inner) = &inlines[0] else {
            panic!("strike: {inlines:?}");
        };
        assert!(inner
            .iter()
            .any(|inline| matches!(inline, Inline::Link { .. })));
    }

    #[test]
    fn traps_stay_literal_through_serialization() {
        let (_dir, projection) =
            host_only("Owe $69.00 and :sparkles: to @ROUTE today.\n");
        let view = syntax_test_view(&projection.parsed.identity, &projection);
        let serialized =
            serialize_projected_markdown(&view).expect("serialize");
        // The contract escapes ASCII punctuation while keeping the text
        // literal for pandoc (no math, emoji, or citations).
        assert!(serialized.contains("\\$69\\.00"), "{serialized}");
        assert!(serialized.contains("\\:sparkles\\:"), "{serialized}");
        assert!(serialized.contains("\\@ROUTE"), "{serialized}");
    }

    #[test]
    fn bare_paths_urls_and_long_tokens_become_code_or_links() {
        let long = "x".repeat(60);
        let (_dir, projection) = host_only(&format!(
            "Open ~/docs/notes/today and {long} plus https://example.test/a-b_c.\n"
        ));
        let Block::Paragraph(inlines) = &projection.blocks[0] else {
            panic!("para: {:?}", projection.blocks);
        };
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Code(code) if code == "~/docs/notes/today"
        )));
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Code(code) if code == &long
        )));
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::Link { target: LinkTarget::Url(url), .. }
            if url == "https://example.test/a-b_c"
        )));
    }

    #[test]
    fn very_long_lines_and_missing_newlines_project() {
        let spaced = (0..300).map(|_| "word").collect::<Vec<_>>().join(" ");
        assert!(spaced.len() > 1400);
        let (_dir, projection) =
            host_only(&format!("{spaced} tail-no-newline"));
        assert_eq!(projection.blocks.len(), 1);
        assert!(matches!(projection.blocks[0], Block::Paragraph(_)));
        let flat = block_shape(&projection.blocks[0]);
        assert!(flat.contains("tail-no-newline"), "{flat}");
    }

    #[test]
    fn unicode_survives_projection_and_serialization() {
        let (_dir, projection) =
            host_only("café — naïve “quotes” {braces} → λ\n");
        let view = syntax_test_view(&projection.parsed.identity, &projection);
        let serialized =
            serialize_projected_markdown(&view).expect("serialize");
        for token in ["café", "—", "naïve", "λ", "→"] {
            assert!(serialized.contains(token), "{serialized}");
        }
    }

    #[test]
    fn raw_source_metadata_survives_projection() {
        let (_dir, projection) = project_host(
            &[(
                "Host.md",
                "---\ntype: '[[project]]'\ncustom: {retain: true}\n---\n\n- [?] blocked [custom:: value] #task ^task-id\n",
            )],
            "Host.md",
        );
        let parsed = &projection.parsed;
        assert_eq!(
            parsed.raw_bytes.as_ref(),
            &fs::read(parsed.identity.canonical_path.clone())
                .expect("host bytes")
        );
        assert_eq!(
            parsed.sha256,
            crate::native::highlights_ref::note_snapshot::classify::sha256_hex(
                &parsed.raw_bytes
            )
        );
        assert_eq!(parsed.task_lines.len(), 1);
        let task = &parsed.task_lines[0];
        assert_eq!(
            task.raw_line,
            "- [?] blocked [custom:: value] #task ^task-id"
        );
        assert_eq!(task.body, "blocked [custom:: value] #task ^task-id");
        assert_eq!(task.block_id, Some("task-id".to_string()));
        assert_eq!(task.extent.first_line, 6);
        assert_eq!(
            parsed.frontmatter.as_ref().expect("frontmatter").metadata
                ["custom"]["retain"],
            serde_yaml::Value::Bool(true)
        );
        // The projected task keeps its status and drops bookkeeping.
        let Block::Task(view) = &projection.blocks[0] else {
            panic!("task: {:?}", projection.blocks);
        };
        assert_eq!(view.status, SnapshotTaskStatus::Blocked);
        assert_eq!(flatten_inlines(&view.content), "blocked custom: value");
    }

    /// Wrap projected blocks in a deterministic single-section view so
    /// syntax output can be checked end to end through the serializer.
    fn syntax_test_view(
        source: &SourceIdentity,
        projection: &SyntaxProjection,
    ) -> SnapshotView {
        let section_anchor = SnapshotAnchor {
            identity: AnchorIdentity {
                source: source.clone(),
                kind: AnchorKind::Heading,
                fragment: "syntax".to_string(),
            },
            id: "syntax".to_string(),
        };
        let mut anchors = vec![section_anchor.clone()];
        anchors.extend(projection.anchors.iter().cloned());
        SnapshotView {
            source: source.clone(),
            masthead: SnapshotMasthead {
                title: "Syntax Coverage".to_string(),
                kicker: vec![Inline::Text("NOTE".to_string())],
                display_title: vec![Inline::Text(
                    "Syntax Coverage".to_string(),
                )],
                dek: None,
                meta: Vec::new(),
                stats: Vec::new(),
                omitted: None,
                snapshot_line: vec![Inline::Text(
                    "Synthetic syntax golden".to_string(),
                )],
                footer: vec![Inline::Text("Generated fixture".to_string())],
                glyph_table: "glyphs.lua".to_string(),
                return_links: false,
            },
            sections: vec![SnapshotSection {
                source: source.clone(),
                heading: SnapshotHeading {
                    level: HeadingLevel::H1,
                    content: vec![Inline::Text("Syntax".to_string())],
                    class: HeadingClass::Normal,
                    anchor: section_anchor,
                },
                blocks: projection.blocks.clone(),
            }],
            closed: None,
            assets: projection.assets.clone(),
            summary: SnapshotSummary::default(),
            warnings: Vec::new(),
            anchors,
        }
    }

    fn assert_syntax_golden(name: &str, actual: &str, vault_name: &str) {
        // Temp vault names are random: normalize the encoded vault before
        // comparing so the golden stays deterministic.
        let actual = actual.replace(
            &format!("vault={}", percent_encode(vault_name)),
            "vault=VAULT",
        );
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/note_snapshot")
            .join(format!("{name}.projected.md"));
        if std::env::var_os("BOB_UPDATE_GOLDENS").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            fs::write(&path, &actual).expect("update syntax golden");
            return;
        }
        let expected = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("read syntax golden {}: {error}", path.display());
        });
        assert_eq!(actual, expected, "syntax golden: {}", path.display());
    }

    /// End-to-end coverage: every syntax construct through projection and
    /// serialization in one deterministic golden.
    #[test]
    fn syntax_coverage_golden() {
        let (directory, projection) = project_host(
            &[
                (
                    "Host.md",
                    "---\ntype: note\n---\n\n\
                     # Coverage Host\n\n\
                     Intro links [[Other|aliased]], [[Other#Details]], [[Other#^t1]], \
                     [[Missing Note]], and [[#Coverage Host]].\n\n\
                     ~~struck [[Other|lifted]] tail~~ and ~~plain `code` strike~~.\n\n\
                     - [ ] Open row [created:: 2026-10-15] #task\n\
                     - [*] Next row [priority:: high] due:: Friday #task ^next-id\n\
                     - [/] Wip row #task\n\
                     - [?] Blocked row #task\n\
                     - [x] Done row #task\n\
                     - [-] Canceled row #task\n\
                     - [!] Odd row #task\n  - [ ] Nested child #keepme\n\
                     \n\
                     A tag #keepme, $12.50, :smile:, and @ROUTE stay literal.\n\n\
                     Keep chip: [💡](https://keep.example.test/k \"Open in Google Keep\").\n\n\
                     Quote chip: [[Other#^h-abc|🔖]] [h:: abc].\n\n\
                     Inline query `= 2 + 2` beside `plain code`.\n\n\
                     Path ~/docs/today plus https://example.test/way autolink.\n\n\
                     kept %% gone %% here\n\n\
                     ![ImageAlt](img/a.png)\n\n\
                     ![[Other]]\n\n\
                     > [!note] A note\n> quoted body\n\n\
                     ```dataview\nTABLE file FROM x\n```\n\n\
                     ```rust\nlet kept = \"[[literal]]\";\n```\n",
                ),
                (
                    "Other.md",
                    "# Other\n\n## Details\n\nDetail body.\n\n- [ ] Other task [custom:: kept] #task ^t1\n\nQuoted line. ^h-abc\n",
                ),
                ("img/a.png", "fake-png-bytes"),
            ],
            "Host.md",
        );
        // No residue from the projection traps in this fixture.
        assert!(warnings_of(&projection, SnapshotWarningKind::AmbiguousLink)
            .is_empty());
        let view = syntax_test_view(&projection.parsed.identity, &projection);
        let serialized =
            serialize_projected_markdown(&view).expect("serialize");
        let vault_name = directory
            .path()
            .canonicalize()
            .expect("canonical")
            .file_name()
            .expect("basename")
            .to_string_lossy()
            .into_owned();
        assert_syntax_golden("syntax-coverage", &serialized, &vault_name);
        super::super::tests::assert_no_obsidian_residue(&serialized);
    }
}
