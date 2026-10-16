//! Note and attachment resolution with Obsidian semantics.
//!
//! [`VaultNotes`] wraps [`VaultLinkResolver`](crate::native::vault_links::VaultLinkResolver):
//! the stripped note part of a content link resolves to one member vault
//! note, with alias and heading/block fragment retained separately. The same
//! vault-membership exclusions as target classification apply, so library and
//! intake directories never shadow real notes through a bare basename.
//!
//! [`FragmentIndex`] is built lazily over cached source bytes and maps one
//! note's headings and block IDs to source extents: heading embeds select
//! through the next equal-or-shallower heading, task/list block embeds
//! include their indented subtree, and paragraph block IDs cover their
//! paragraph. Frontmatter, fenced code, and comments never contribute
//! headings or block IDs. A missing or ambiguous fragment is a typed lookup
//! result for the caller to report as a diagnostic, never an invented anchor.
//!
//! [`AttachmentIndex`] resolves image-style targets with the approved
//! precedence (exact vault-relative path, then relative to the source note
//! directory, then unique basename) and owns deterministic scratch-relative
//! asset naming with canonical-path deduplication. Copying into scratch
//! belongs to create-wiring.
//!
//! The `obsidian://` builders share one percent-encoding/template primitive
//! with companion audio: snapshot links use the default template while audio
//! keeps its configured template, empty-template handling, and workflow
//! rules (see `create.rs`, which renders through these helpers).

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fmt, fs,
    path::{Path, PathBuf},
};

use super::classify::vault_member_relative;
use super::view::{AssetMediaKind, SnapshotAsset};
use crate::native::vault_links::VaultLinkResolver;

/// Default `obsidian://` template shared by snapshot links and (unless
/// overridden) companion audio.
pub(crate) const DEFAULT_OBSIDIAN_URI_TEMPLATE: &str =
    "obsidian://open?vault={vault}&file={path}";

/// Print text width underpinning `![[img.png|400]]`-style hints: both the
/// snapshot serializer and the print profile agree on this measure, so a
/// pixel hint converts to one deterministic percentage.
pub(crate) const SNAPSHOT_TEXT_WIDTH_PX: u32 = 640;

/// Byte-wise percent-encoding for `obsidian://` components: every byte
/// outside the URI unreserved set becomes `%XX` (uppercase hex). Slashes,
/// spaces, `&`, `#`, `%`, `+`, and non-ASCII bytes are all encoded, so an
/// encoded vault-relative path round-trips through one template slot.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Substitute an already-resolved `{vault}`/`{path}` template. Precedence
/// (environment, then config, then the default template) is resolved by the
/// caller via [`resolve_uri_template`]; an empty template stays empty so
/// audio can disable its link without a special case.
pub(crate) fn render_uri_template(
    template: &str,
    vault: &str,
    path: &str,
) -> String {
    template
        .replace("{vault}", &percent_encode(vault))
        .replace("{path}", &percent_encode(path))
}

/// Template precedence for `obsidian://` links: the environment value wins
/// (even when empty), then the configured value, then `default_template`.
pub(crate) fn resolve_uri_template(
    env_template: Option<&OsStr>,
    configured: Option<&str>,
    default_template: &str,
) -> String {
    env_template.map_or_else(
        || configured.unwrap_or(default_template).to_string(),
        |value| value.to_string_lossy().into_owned(),
    )
}

/// A default-template `obsidian://` link to a vault-relative file target.
/// `file` keeps its vault-relative form (including any `.md` suffix), so
/// notes and attachments share one lossless encoding with audio.
pub(crate) fn obsidian_uri(vault: &str, file: &str) -> String {
    render_uri_template(DEFAULT_OBSIDIAN_URI_TEMPLATE, vault, file)
}

/// A default-template `obsidian://` link retaining an external
/// heading/block fragment inside the encoded file target. Callers pass
/// `None` for same-host-note fragments, which resolve to internal anchors
/// later instead of self-links.
pub(crate) fn obsidian_uri_with_fragment(
    vault: &str,
    file: &str,
    fragment: Option<&LinkFragment>,
) -> String {
    let mut target = file.to_string();
    if let Some(fragment) = fragment {
        target.push('#');
        target.push_str(&fragment.encode_form());
    }
    obsidian_uri(vault, &target)
}

/// The fragment half of an Obsidian link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkFragment {
    /// `#Heading` (kept verbatim; matching is case-insensitive).
    Heading(String),
    /// `#^block-id`.
    Block(String),
}

impl LinkFragment {
    /// Form used inside an encoded `obsidian://` file target.
    pub(crate) fn encode_form(&self) -> String {
        match self {
            Self::Heading(heading) => heading.clone(),
            Self::Block(id) => format!("^{id}"),
        }
    }
}

/// A parsed content-link target: stripped note part plus separately
/// retained alias and heading/block fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedLinkTarget {
    /// Note part with wrapper, alias, and fragment removed. Empty means
    /// the link addresses its own host note.
    pub note: String,
    pub alias: Option<String>,
    pub fragment: Option<LinkFragment>,
}

/// Normalize one link target: unwrap a single `[[...]]` wrapper, split the
/// `|alias`, split the `#heading`/`#^id` fragment, and trim each part. An
/// empty fragment (`[[note#]]`) is dropped. The `.md` suffix (if any) is
/// left for the resolver, which applies it deterministically.
pub(crate) fn parse_link_target(raw: &str) -> ParsedLinkTarget {
    let mut text = raw.trim();
    if let Some(inner) = text.strip_prefix("[[") {
        if let Some(stripped) = inner.strip_suffix("]]") {
            text = stripped;
        }
    }
    let text = text.trim();
    let (before_alias, alias) = match text.split_once('|') {
        Some((note, alias)) => {
            let alias = alias.trim();
            (note.trim(), (!alias.is_empty()).then(|| alias.to_string()))
        }
        None => (text, None),
    };
    let (note, fragment) = match before_alias.split_once('#') {
        Some((note, fragment)) => {
            let fragment = fragment.trim();
            let fragment = if fragment.is_empty() {
                None
            } else if let Some(id) = fragment.strip_prefix('^') {
                let id = id.trim();
                (!id.is_empty()).then(|| LinkFragment::Block(id.to_string()))
            } else {
                Some(LinkFragment::Heading(fragment.to_string()))
            };
            (note.trim(), fragment)
        }
        None => (before_alias, None),
    };
    ParsedLinkTarget {
        note: note.to_string(),
        alias,
        fragment,
    }
}

/// Typed note-resolution failures for actionable diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteResolveError {
    /// No member note answers the target, with up to three near stems.
    Missing {
        display: String,
        suggestions: Vec<String>,
    },
    /// Several member notes share the basename, with the sorted complete
    /// candidate list (vault-relative, `/`-joined).
    Ambiguous {
        display: String,
        candidates: Vec<PathBuf>,
    },
}

impl fmt::Display for NoteResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing {
                display,
                suggestions,
            } => {
                write!(formatter, "no file or vault note named '{display}'")?;
                if !suggestions.is_empty() {
                    let quoted = suggestions
                        .iter()
                        .map(|stem| format!("'{stem}'"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(formatter, "\nhint: did you mean {quoted}?")?;
                }
                Ok(())
            }
            Self::Ambiguous {
                display,
                candidates,
            } => {
                write!(formatter, "multiple vault notes named '{display}':")?;
                for candidate in candidates {
                    write!(
                        formatter,
                        "\n  {}",
                        candidate.to_string_lossy().replace('\\', "/")
                    )?;
                }
                write!(
                    formatter,
                    "\nhint: pass a vault-relative path instead of a bare name"
                )
            }
        }
    }
}

impl std::error::Error for NoteResolveError {}

/// One resolved content-link note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedNote {
    pub canonical_path: PathBuf,
    pub vault_relative_path: PathBuf,
    /// True when the target named the host note itself (empty note part).
    pub same_note: bool,
}

/// Vault membership plus Obsidian note resolution over cached directory
/// state. Construct once per projection operation and share it with every
/// link the projection resolves.
pub(crate) struct VaultNotes {
    root: PathBuf,
    vault_name: String,
    resolver: VaultLinkResolver,
}

impl VaultNotes {
    pub(crate) fn new(root: PathBuf, vault_name: String) -> Self {
        let resolver = VaultLinkResolver::new(root.clone());
        Self {
            root,
            vault_name,
            resolver,
        }
    }

    pub(crate) fn vault_name(&self) -> &str {
        &self.vault_name
    }

    /// Canonicalize `candidate` and enforce vault containment plus every
    /// directory exclusion, returning the vault-relative path on success.
    /// Symlink escapes fail containment; non-existent paths fail
    /// canonicalization.
    pub(crate) fn member_relative(&self, candidate: &Path) -> Option<PathBuf> {
        let canonical = candidate.canonicalize().ok()?;
        vault_member_relative(&self.root, &canonical)
    }

    fn member_canonical(&self, relative: &Path) -> Option<PathBuf> {
        let canonical = self.root.join(relative).canonicalize().ok()?;
        vault_member_relative(&self.root, &canonical)?;
        Some(canonical)
    }

    /// Resolve one parsed link target against the vault. `current_relative`
    /// is the host note's vault-relative path (used only for error context;
    /// basename fallback is vault-wide). Targets whose only matches live in
    /// excluded directories report missing, keeping intake/library files
    /// from shadowing real notes.
    pub(crate) fn resolve_note(
        &self,
        current_relative: &Path,
        parsed: &ParsedLinkTarget,
    ) -> Result<ResolvedNote, NoteResolveError> {
        if parsed.note.is_empty() {
            let canonical =
                self.root.join(current_relative).canonicalize().map_err(
                    |_| NoteResolveError::Missing {
                        display: fragment_display(parsed),
                        suggestions: Vec::new(),
                    },
                )?;
            let relative = canonical
                .strip_prefix(&self.root)
                .map(Path::to_path_buf)
                .map_err(|_| NoteResolveError::Missing {
                    display: fragment_display(parsed),
                    suggestions: Vec::new(),
                })?;
            return Ok(ResolvedNote {
                canonical_path: canonical,
                vault_relative_path: relative,
                same_note: true,
            });
        }
        let display = parsed.note.clone();
        let mut candidates = self
            .resolver
            .candidates(&parsed.note)
            .into_iter()
            .filter_map(|relative| {
                self.member_canonical(&relative)
                    .map(|canonical| (canonical, relative))
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.len() {
            0 => {
                // An excluded on-disk hit is not a vault note for content
                // links: report it missing rather than inventing a target.
                Err(NoteResolveError::Missing {
                    display,
                    suggestions: self.member_suggestions(&parsed.note),
                })
            }
            1 => {
                let (canonical_path, vault_relative_path) =
                    candidates.into_iter().next().expect("counted one");
                Ok(ResolvedNote {
                    canonical_path,
                    vault_relative_path,
                    same_note: false,
                })
            }
            _ => Err(NoteResolveError::Ambiguous {
                display,
                candidates: candidates
                    .into_iter()
                    .map(|(_, relative)| relative)
                    .collect(),
            }),
        }
    }

    /// Near-stem suggestions restricted to stems with at least one member
    /// note, so hints never point into excluded directories.
    fn member_suggestions(&self, note: &str) -> Vec<String> {
        self.resolver
            .near_stems(note, 3)
            .into_iter()
            .filter(|stem| {
                self.resolver
                    .candidates(stem)
                    .iter()
                    .any(|relative| self.member_canonical(relative).is_some())
            })
            .collect()
    }

    /// Exact-hit probe for target classification: distinguishes an
    /// excluded on-disk path (ordinary Markdown) from a genuinely unknown
    /// name. Returns the canonical path when the raw note part names an
    /// existing vault file, member or not.
    pub(crate) fn existing_vault_file(
        &self,
        parsed: &ParsedLinkTarget,
    ) -> Option<PathBuf> {
        if parsed.note.is_empty() {
            return None;
        }
        self.resolver.candidates(&parsed.note).into_iter().find_map(
            |relative| {
                let canonical =
                    self.root.join(&relative).canonicalize().ok()?;
                if canonical == self.root || !canonical.starts_with(&self.root)
                {
                    return None;
                }
                canonical.is_file().then_some(canonical)
            },
        )
    }
}

fn fragment_display(parsed: &ParsedLinkTarget) -> String {
    match &parsed.fragment {
        Some(LinkFragment::Heading(heading)) => format!("#{heading}"),
        Some(LinkFragment::Block(id)) => format!("#^{id}"),
        None => String::new(),
    }
}

/// What a fragment lookup found: headings key on lowercased trimmed text,
/// block IDs on their exact `^id` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FragmentKind {
    Heading { level: usize },
    Block,
}

/// One indexed fragment with its selected source extent (one-based,
/// inclusive line numbers plus byte offsets into the cached text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FragmentSpan {
    pub kind: FragmentKind,
    /// Display label: the heading text or `^block-id`.
    pub label: String,
    pub first_line: usize,
    pub last_line: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Source-local heading/block lookup over one note's cached text.
/// Frontmatter, fenced code, and comments never contribute entries, so
/// fragments always address real prose. Forward references work because
/// the whole note is indexed before any lookup runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FragmentIndex {
    headings: BTreeMap<String, Vec<FragmentSpan>>,
    blocks: BTreeMap<String, Vec<FragmentSpan>>,
}

/// Outcome of one fragment lookup. Missing and ambiguous fragments are
/// diagnostics for the caller to report, never invented anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FragmentLookup {
    Found(FragmentSpan),
    Missing,
    Ambiguous { count: usize },
}

impl FragmentIndex {
    /// Look up one parsed fragment.
    pub(crate) fn lookup(&self, fragment: &LinkFragment) -> FragmentLookup {
        let spans = match fragment {
            LinkFragment::Heading(heading) => {
                self.headings.get(&normalize_heading_key(heading))
            }
            LinkFragment::Block(id) => self.blocks.get(id),
        };
        match spans {
            None => FragmentLookup::Missing,
            Some(spans) if spans.len() == 1 => {
                FragmentLookup::Found(spans[0].clone())
            }
            Some(spans) => FragmentLookup::Ambiguous { count: spans.len() },
        }
    }
}

/// Shared heading key: trimmed, case-insensitive. Both the index builder
/// and link resolution use this, so `[[note#requirements]]` finds
/// `## Requirements`.
pub(crate) fn normalize_heading_key(heading: &str) -> String {
    heading.trim().to_lowercase()
}

/// Tabs count to this display width for indented-subtree comparisons.
const INDENT_TAB_WIDTH: usize = 4;

fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for character in line.chars() {
        if character == ' ' {
            width += 1;
        } else if character == '\t' {
            width += INDENT_TAB_WIDTH - (width % INDENT_TAB_WIDTH);
        } else {
            break;
        }
    }
    width
}

fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

/// True for `- item`, `* item`, `+ item`, `1. item`, `1) item`, and their
/// checkbox forms (`- [x] item`). The content after the marker must be
/// non-empty; `-` alone (a thematic break) is not a list item.
fn is_list_item_line(line: &str) -> bool {
    let Some(after) = strip_list_marker(line.trim_start()) else {
        return false;
    };
    !after.trim().is_empty()
}

fn strip_list_marker(line: &str) -> Option<&str> {
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* ").or_else(|| line.strip_prefix("+ ")))
    {
        return Some(rest);
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(stripped) =
            rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
        {
            return Some(stripped);
        }
    }
    None
}

/// Line ranges hidden from fragment indexing: fenced code plus multiline
/// `%%...%%` and `<!--...-->` comments. A line holding both halves of an
/// inline comment pair stays visible; only its comment text is stripped
/// from heading keys (see [`strip_inline_comment_text`]).
fn hidden_line_ranges(lines: &[&str]) -> BTreeSet<usize> {
    use crate::native::markdown::fenced_lines;
    let mut hidden = fenced_lines(lines, 0..lines.len());
    hidden.extend(super::obsidian::hidden_comment_lines(lines));
    hidden
}

/// Drop one inline `%%...%%` pair's text from a heading title so a
/// trailing comment cannot fork the heading key. Text after the pair is
/// kept; an unclosed `%%` leaves the title untouched.
fn strip_inline_comment_text(line: &str) -> String {
    super::obsidian::strip_comments_for_fragment(line)
}

/// Build the fragment index for one note's cached text. Only ATX headings
/// are indexed; a heading carrying a trailing `^block-id` registers under
/// both its heading text and that block ID.
pub(crate) fn build_fragment_index(text: &str) -> FragmentIndex {
    let lines = text.lines().collect::<Vec<_>>();
    let line_starts = line_byte_starts(text, lines.len());
    let frontmatter_end =
        crate::native::markdown::strictly_closed_frontmatter_end(&lines)
            .map(|end| end + 1)
            .unwrap_or(0);
    let hidden = hidden_line_ranges(&lines);

    let mut index = FragmentIndex::default();
    let mut heading_lines = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if number < frontmatter_end || hidden.contains(&number) {
            continue;
        }
        if let Some((level, title)) = crate::native::markdown::atx_heading(line)
        {
            let stripped = strip_inline_comment_text(title);
            let trimmed = stripped.trim_end();
            // A heading's own trailing `^block-id` addresses the heading
            // without becoming part of its link text; the generic block-ID
            // scan below still registers the ID itself.
            let block_on_heading =
                crate::native::collect_done::trailing_block_id_in_line(trimmed);
            let title = match &block_on_heading {
                Some(id) => trimmed
                    .strip_suffix(id.as_str())
                    .and_then(|text| text.strip_suffix('^'))
                    .unwrap_or(trimmed)
                    .trim_end(),
                None => trimmed.trim(),
            };
            if title.is_empty() {
                continue;
            }
            heading_lines.push((number, level));
            let key = normalize_heading_key(title);
            index.headings.entry(key).or_default().push(FragmentSpan {
                kind: FragmentKind::Heading { level },
                label: title.to_string(),
                first_line: number + 1,
                last_line: number + 1,
                byte_start: line_starts[number],
                byte_end: line_starts[number] + line.len(),
            });
        }
        if let Some(id) =
            crate::native::collect_done::trailing_block_id_in_line(line)
        {
            index
                .blocks
                .entry(id.clone())
                .or_default()
                .push(FragmentSpan {
                    kind: FragmentKind::Block,
                    label: format!("^{id}"),
                    first_line: number + 1,
                    last_line: number + 1,
                    byte_start: line_starts[number],
                    byte_end: line_starts[number] + line.len(),
                });
        }
    }

    finalize_heading_extents(&mut index, &lines, &line_starts, heading_lines);
    finalize_block_extents(&mut index, &lines, &line_starts, hidden);
    index
}

fn line_byte_starts(text: &str, count: usize) -> Vec<usize> {
    let mut starts = Vec::with_capacity(count);
    let mut offset = 0;
    for chunk in text.split_inclusive('\n').take(count) {
        starts.push(offset);
        offset += chunk.len();
    }
    // A final line without a trailing newline still needs a start offset.
    while starts.len() < count {
        starts.push(offset);
    }
    starts
}

/// Extend each heading span through the next equal-or-shallower heading
/// (exclusive), trimming trailing blank lines.
fn finalize_heading_extents(
    index: &mut FragmentIndex,
    lines: &[&str],
    line_starts: &[usize],
    heading_lines: Vec<(usize, usize)>,
) {
    for (position, (number, level)) in heading_lines.iter().enumerate() {
        let mut end = lines.len();
        for (later_number, later_level) in
            heading_lines.iter().skip(position + 1)
        {
            if *later_level <= *level {
                end = *later_number;
                break;
            }
        }
        let mut last = end;
        while last > number + 1 && is_blank_line(lines[last - 1]) {
            last -= 1;
        }
        let byte_end = if last >= lines.len() {
            text_end_byte(line_starts, lines)
        } else {
            line_starts[last]
        };
        // `last` is the exclusive 0-based end, so it already reads as the
        // inclusive 1-based last line.
        for spans in index.headings.values_mut() {
            for span in spans.iter_mut() {
                if span.first_line == number + 1 {
                    span.last_line = last.max(number + 1);
                    span.byte_end = byte_end.max(span.byte_end);
                }
            }
        }
    }
}

/// Extend each block-ID span: list/task items absorb their indented
/// subtree, other lines absorb their contiguous paragraph.
fn finalize_block_extents(
    index: &mut FragmentIndex,
    lines: &[&str],
    line_starts: &[usize],
    hidden: BTreeSet<usize>,
) {
    for spans in index.blocks.values_mut() {
        for span in spans.iter_mut() {
            let number = span.first_line - 1;
            let line = lines.get(number).copied().unwrap_or("");
            let (first, last) = if is_list_item_line(line) {
                list_subtree_extent(lines, number, &hidden)
            } else {
                paragraph_extent(lines, number, &hidden)
            };
            span.first_line = first + 1;
            span.last_line = last.max(first) + 1;
            span.byte_start = line_starts[first];
            span.byte_end = if last + 1 >= lines.len() {
                text_end_byte(line_starts, lines)
            } else {
                line_starts[last + 1]
            };
        }
    }
}

fn text_end_byte(line_starts: &[usize], lines: &[&str]) -> usize {
    line_starts
        .last()
        .copied()
        .unwrap_or(0)
        .saturating_add(lines.last().map_or(0, |line| line.len()))
}

/// A list item plus every deeper-indented (or blank-padded) following line.
fn list_subtree_extent(
    lines: &[&str],
    number: usize,
    hidden: &BTreeSet<usize>,
) -> (usize, usize) {
    let base = indent_width(lines[number]);
    let mut last = number;
    for (index, line) in lines.iter().enumerate().skip(number + 1) {
        if hidden.contains(&index) {
            break;
        }
        if is_blank_line(line) {
            // A blank line belongs to the subtree only when deeper content
            // follows; peek ahead past consecutive blanks.
            let deeper_follows = lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, next)| !is_blank_line(next))
                .is_some_and(|(next_index, next)| {
                    !hidden.contains(&next_index) && indent_width(next) > base
                });
            if deeper_follows {
                last = index;
                continue;
            }
            break;
        }
        if crate::native::markdown::atx_heading(line).is_some() {
            break;
        }
        if indent_width(line) > base {
            last = index;
        } else {
            break;
        }
    }
    (number, last)
}

/// The contiguous non-blank, non-heading run around one line.
fn paragraph_extent(
    lines: &[&str],
    number: usize,
    hidden: &BTreeSet<usize>,
) -> (usize, usize) {
    let mut first = number;
    while first > 0 {
        let previous = first - 1;
        if hidden.contains(&previous)
            || is_blank_line(lines[previous])
            || crate::native::markdown::atx_heading(lines[previous]).is_some()
        {
            break;
        }
        first = previous;
    }
    let mut last = number;
    while last + 1 < lines.len() {
        let next = last + 1;
        if hidden.contains(&next)
            || is_blank_line(lines[next])
            || crate::native::markdown::atx_heading(lines[next]).is_some()
        {
            break;
        }
        last = next;
    }
    (first, last)
}

/// Convert a [`NoteResolveError`] into its target-classification twin so
/// both layers report one diagnostic shape.
pub(crate) fn classify_error_from_resolve(
    display_default: &str,
    error: NoteResolveError,
) -> super::classify::ClassifyError {
    match error {
        NoteResolveError::Missing {
            display,
            suggestions,
        } => super::classify::ClassifyError::Missing {
            display: if display.is_empty() {
                display_default.to_string()
            } else {
                display
            },
            suggestions,
        },
        NoteResolveError::Ambiguous {
            display,
            candidates,
        } => super::classify::ClassifyError::Ambiguous {
            display,
            candidates: candidates
                .iter()
                .map(|candidate| candidate.to_string_lossy().replace('\\', "/"))
                .collect(),
        },
    }
}

/// Convert a positive pixel width hint to a capped percentage of
/// [`SNAPSHOT_TEXT_WIDTH_PX`]. Zero or negative-range hints (`0`) carry no
/// width. `400` maps to `62`, matching the projection contract fixture.
pub(crate) fn width_hint_to_percent(width_px: u32) -> Option<u8> {
    if width_px == 0 {
        return None;
    }
    let percent = u64::from(width_px) * 100 / u64::from(SNAPSHOT_TEXT_WIDTH_PX);
    Some(percent.min(100) as u8)
}

/// An image-style target with its optional `|width` hint separated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAttachmentTarget {
    /// Attachment path with wrapper and width hint removed.
    pub path: String,
    pub width_px: Option<u32>,
}

/// Normalize one attachment target: unwrap a single `[[...]]` wrapper,
/// then split a trailing `|400` / `|400x300` width hint. Any other `|`
/// suffix stays part of the path.
pub(crate) fn parse_attachment_target(raw: &str) -> ParsedAttachmentTarget {
    let mut text = raw.trim();
    if let Some(inner) = text.strip_prefix("[[") {
        if let Some(stripped) = inner.strip_suffix("]]") {
            text = stripped;
        }
    }
    let text = text.trim();
    match text.rsplit_once('|') {
        Some((path, hint)) if is_width_hint(hint) => ParsedAttachmentTarget {
            path: path.trim().to_string(),
            width_px: hint
                .trim()
                .split(['x', 'X'])
                .next()
                .and_then(|width| width.parse::<u32>().ok()),
        },
        _ => ParsedAttachmentTarget {
            path: text.to_string(),
            width_px: None,
        },
    }
}

/// True for `400` and `400x300` width hints: all-digit runs with an
/// optional all-digit `x`-separated height.
fn is_width_hint(hint: &str) -> bool {
    fn all_digits(text: &str) -> bool {
        !text.trim().is_empty()
            && text.trim().bytes().all(|byte| byte.is_ascii_digit())
    }
    match hint.trim().split_once(['x', 'X']) {
        Some((width, height)) => all_digits(width) && all_digits(height),
        None => all_digits(hint),
    }
}

/// One resolved attachment: provenance for the later render-scratch copy
/// plus the parsed display hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAttachment {
    pub canonical_source_path: PathBuf,
    pub vault_relative_path: PathBuf,
    pub media_kind: AssetMediaKind,
    pub width_px_hint: Option<u32>,
}

/// Typed attachment-resolution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachmentResolutionError {
    /// No vault file answers the target.
    Missing { display: String },
    /// Several vault files share the basename, with the sorted complete
    /// candidate list (vault-relative, `/`-joined).
    Ambiguous {
        display: String,
        candidates: Vec<String>,
    },
}

impl fmt::Display for AttachmentResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { display } => {
                write!(formatter, "no vault attachment named '{display}'")
            }
            Self::Ambiguous {
                display,
                candidates,
            } => {
                write!(
                    formatter,
                    "multiple vault attachments named '{display}':"
                )?;
                for candidate in candidates {
                    write!(formatter, "\n  {candidate}")?;
                }
                write!(
                    formatter,
                    "\nhint: pass a vault-relative path instead of a bare name"
                )
            }
        }
    }
}

impl std::error::Error for AttachmentResolutionError {}

fn media_kind_for_path(path: &Path) -> AssetMediaKind {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => AssetMediaKind::Png,
        "jpg" | "jpeg" => AssetMediaKind::Jpeg,
        "gif" => AssetMediaKind::Gif,
        "webp" => AssetMediaKind::Webp,
        "svg" => AssetMediaKind::Svg,
        _ => AssetMediaKind::Other,
    }
}

#[derive(Debug, Default)]
struct BuiltAttachmentIndex {
    /// Lowercased file name to canonical paths, each list sorted.
    by_basename: BTreeMap<String, Vec<PathBuf>>,
}

/// Lazily built vault attachment index with the approved precedence:
/// exact vault-relative path, then relative to the source note directory,
/// then unique basename. Construction performs no I/O; the vault walk
/// runs on the first basename search and is reused afterwards. Only
/// dot-directories are skipped, mirroring note basename search.
pub(crate) struct AttachmentIndex {
    root: PathBuf,
    built: Option<BuiltAttachmentIndex>,
    assets: BTreeMap<PathBuf, SnapshotAsset>,
}

impl AttachmentIndex {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            built: None,
            assets: BTreeMap::new(),
        }
    }

    /// Resolve one parsed attachment target from `source_note_relative`'s
    /// directory. Every returned path is canonicalized and contained in
    /// the vault, so symlink escapes can only fail, never leak.
    pub(crate) fn resolve(
        &mut self,
        source_note_relative: &Path,
        parsed: &ParsedAttachmentTarget,
    ) -> Result<ResolvedAttachment, AttachmentResolutionError> {
        let display = parsed.path.clone();
        if display.is_empty() {
            return Err(AttachmentResolutionError::Missing { display });
        }
        let target_path = Path::new(&display);
        if let Some(found) = self.exact_vault_hit(target_path) {
            return Ok(self.describe(found, parsed.width_px));
        }
        if let Some(parent) = source_note_relative.parent() {
            if let Some(found) = self.exact_vault_hit(&parent.join(target_path))
            {
                return Ok(self.describe(found, parsed.width_px));
            }
        }
        if target_path.components().count() > 1 {
            return Err(AttachmentResolutionError::Missing { display });
        }
        let index = self.index();
        let needle = display.to_lowercase();
        let candidates =
            index.by_basename.get(&needle).cloned().unwrap_or_default();
        match candidates.len() {
            0 => Err(AttachmentResolutionError::Missing { display }),
            1 => {
                let canonical = candidates.into_iter().next().expect("one");
                Ok(self.describe(canonical, parsed.width_px))
            }
            _ => Err(AttachmentResolutionError::Ambiguous {
                display,
                candidates: candidates
                    .iter()
                    .map(|canonical| {
                        canonical
                            .strip_prefix(&self.root)
                            .map(|relative| {
                                relative.to_string_lossy().replace('\\', "/")
                            })
                            .unwrap_or_else(|_| {
                                canonical.to_string_lossy().replace('\\', "/")
                            })
                    })
                    .collect(),
            }),
        }
    }

    /// Register one resolved attachment, deduplicating by canonical source
    /// path: repeats return the same [`SnapshotAsset`] (same id, same
    /// scratch-relative name, first hint wins for the percentage).
    pub(crate) fn register_asset(
        &mut self,
        attachment: &ResolvedAttachment,
    ) -> SnapshotAsset {
        if let Some(existing) =
            self.assets.get(&attachment.canonical_source_path)
        {
            return existing.clone();
        }
        let sequence = self.assets.len() + 1;
        let extension = attachment
            .canonical_source_path
            .extension()
            .and_then(OsStr::to_str)
            .map(sanitize_asset_extension)
            .filter(|extension| !extension.is_empty())
            .unwrap_or_else(|| "bin".to_string());
        let asset = SnapshotAsset {
            id: format!("a{sequence}"),
            canonical_source_path: attachment.canonical_source_path.clone(),
            relative_path: PathBuf::from(format!(
                "assets/{sequence}.{extension}"
            )),
            media_kind: attachment.media_kind.clone(),
            width_px_hint: attachment.width_px_hint,
            width_percent: attachment
                .width_px_hint
                .and_then(width_hint_to_percent),
        };
        self.assets
            .insert(attachment.canonical_source_path.clone(), asset.clone());
        asset
    }

    fn describe(
        &self,
        canonical: PathBuf,
        width_px: Option<u32>,
    ) -> ResolvedAttachment {
        let vault_relative_path = canonical
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| canonical.clone());
        let media_kind = media_kind_for_path(&canonical);
        ResolvedAttachment {
            canonical_source_path: canonical,
            vault_relative_path,
            media_kind,
            width_px_hint: width_px,
        }
    }

    fn exact_vault_hit(&self, candidate: &Path) -> Option<PathBuf> {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let canonical = joined.canonicalize().ok()?;
        if canonical == self.root || !canonical.starts_with(&self.root) {
            return None;
        }
        canonical.is_file().then_some(canonical)
    }

    fn index(&mut self) -> &BuiltAttachmentIndex {
        if self.built.is_none() {
            self.built = Some(build_attachment_index(&self.root));
        }
        self.built.as_ref().expect("just built")
    }
}

fn sanitize_asset_extension(extension: &str) -> String {
    extension
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn build_attachment_index(root: &Path) -> BuiltAttachmentIndex {
    let mut index = BuiltAttachmentIndex::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                let Ok(canonical) = path.canonicalize() else {
                    continue;
                };
                if !canonical.starts_with(root) {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(OsStr::to_str) {
                    index
                        .by_basename
                        .entry(name.to_lowercase())
                        .or_default()
                        .push(canonical);
                }
            }
        }
    }
    for candidates in index.by_basename.values_mut() {
        candidates.sort();
        candidates.dedup();
    }
    index
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn synthetic_vault(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp vault");
        for (relative, contents) in files {
            let path = directory.path().join(relative);
            fs::create_dir_all(path.parent().expect("parent"))
                .expect("create parent");
            fs::write(&path, contents).expect("write file");
        }
        directory
    }

    fn vault_notes(directory: &tempfile::TempDir) -> (PathBuf, VaultNotes) {
        let root = directory.path().canonicalize().expect("canonical");
        let notes = VaultNotes::new(root.clone(), "vault".to_string());
        (root, notes)
    }

    fn resolve(
        notes: &VaultNotes,
        host: &str,
        raw: &str,
    ) -> Result<ResolvedNote, NoteResolveError> {
        notes.resolve_note(Path::new(host), &parse_link_target(raw))
    }

    #[test]
    fn snapshot_uris_match_audio_uri_encoding() {
        // The exact Play-URI shape `create.rs` renders for companion audio.
        assert_eq!(
            render_uri_template(
                DEFAULT_OBSIDIAN_URI_TEMPLATE,
                "bob",
                "lib/chat/report.mp3"
            ),
            "obsidian://open?vault=bob&file=lib%2Fchat%2Freport.mp3"
        );
        assert_eq!(
            obsidian_uri("Research Vault", "Notes/My Note.md"),
            "obsidian://open?vault=Research%20Vault&file=Notes%2FMy%20Note.md"
        );
        // Unicode, spaces, and every reserved mark encode; `~` survives.
        assert_eq!(
            percent_encode("café & #100% +x/ünï~"),
            "caf%C3%A9%20%26%20%23100%25%20%2Bx%2F%C3%BCn%C3%AF~"
        );
        // External fragments stay inside the encoded file target; a
        // same-host fragment is the caller's `None`, never a self-link.
        assert_eq!(
            obsidian_uri_with_fragment(
                "bob",
                "Note.md",
                Some(&LinkFragment::Heading("Next Steps".to_string()))
            ),
            "obsidian://open?vault=bob&file=Note.md%23Next%20Steps"
        );
        assert_eq!(
            obsidian_uri_with_fragment(
                "bob",
                "Note.md",
                Some(&LinkFragment::Block("t1".to_string()))
            ),
            "obsidian://open?vault=bob&file=Note.md%23%5Et1"
        );
        assert_eq!(
            obsidian_uri_with_fragment("bob", "Note.md", None),
            "obsidian://open?vault=bob&file=Note.md"
        );
    }

    #[test]
    fn template_precedence_is_env_then_config_then_default() {
        use std::ffi::OsStr;
        assert_eq!(
            resolve_uri_template(
                Some(OsStr::new("custom:{vault}:{path}")),
                Some("configured"),
                DEFAULT_OBSIDIAN_URI_TEMPLATE
            ),
            "custom:{vault}:{path}"
        );
        assert_eq!(
            resolve_uri_template(
                None,
                Some("configured"),
                DEFAULT_OBSIDIAN_URI_TEMPLATE
            ),
            "configured"
        );
        assert_eq!(
            resolve_uri_template(None, None, DEFAULT_OBSIDIAN_URI_TEMPLATE),
            DEFAULT_OBSIDIAN_URI_TEMPLATE
        );
        // An empty environment template disables the link, as audio needs.
        assert_eq!(
            resolve_uri_template(
                Some(OsStr::new("")),
                Some("configured"),
                DEFAULT_OBSIDIAN_URI_TEMPLATE
            ),
            ""
        );
    }

    #[test]
    fn width_hints_convert_on_the_shared_print_measure() {
        assert_eq!(width_hint_to_percent(0), None);
        assert_eq!(width_hint_to_percent(400), Some(62));
        assert_eq!(width_hint_to_percent(640), Some(100));
        assert_eq!(width_hint_to_percent(6400), Some(100));
    }

    #[test]
    fn link_targets_split_wrapper_alias_and_fragment() {
        assert_eq!(
            parse_link_target("[[Deep Note|read it]]"),
            ParsedLinkTarget {
                note: "Deep Note".to_string(),
                alias: Some("read it".to_string()),
                fragment: None,
            }
        );
        assert_eq!(
            parse_link_target("note#Heading Words"),
            ParsedLinkTarget {
                note: "note".to_string(),
                alias: None,
                fragment: Some(LinkFragment::Heading(
                    "Heading Words".to_string()
                )),
            }
        );
        assert_eq!(
            parse_link_target("[[note#^block-id|alias]]"),
            ParsedLinkTarget {
                note: "note".to_string(),
                alias: Some("alias".to_string()),
                fragment: Some(LinkFragment::Block("block-id".to_string())),
            }
        );
        assert_eq!(
            parse_link_target("#Same Host"),
            ParsedLinkTarget {
                note: String::new(),
                alias: None,
                fragment: Some(LinkFragment::Heading("Same Host".to_string())),
            }
        );
        // An empty fragment is dropped; the `.md` suffix stays for the
        // resolver, which applies it deterministically.
        assert_eq!(
            parse_link_target("  [[note#]]  "),
            ParsedLinkTarget {
                note: "note".to_string(),
                alias: None,
                fragment: None,
            }
        );
        assert_eq!(parse_link_target("note.md").note, "note.md");
        // An unclosed wrapper is literal text, honestly unresolvable.
        assert_eq!(parse_link_target("[[unclosed").note, "[[unclosed");
    }

    #[test]
    fn note_resolution_prefers_exact_paths_then_unique_basenames() {
        let directory = synthetic_vault(&[
            ("Projects/Alpha.md", "# Alpha\n"),
            ("Archive/alpha.md", "# other\n"),
            ("Solo.md", "# Solo\n"),
        ]);
        let (_root, notes) = vault_notes(&directory);

        let resolved =
            resolve(&notes, "Daily/2026.md", "Projects/Alpha").expect("exact");
        assert_eq!(
            resolved.vault_relative_path,
            PathBuf::from("Projects/Alpha.md")
        );
        assert!(!resolved.same_note);

        let resolved =
            resolve(&notes, "Daily/2026.md", "solo").expect("basename");
        assert_eq!(resolved.vault_relative_path, PathBuf::from("Solo.md"));

        let error = resolve(&notes, "Daily/2026.md", "Alpha").expect_err("dup");
        let NoteResolveError::Ambiguous {
            display,
            candidates,
        } = error
        else {
            panic!("expected ambiguity");
        };
        assert_eq!(display, "Alpha");
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("Archive/alpha.md"),
                PathBuf::from("Projects/Alpha.md"),
            ]
        );
        let rendered = NoteResolveError::Ambiguous {
            display: display.clone(),
            candidates: candidates.clone(),
        }
        .to_string();
        assert!(rendered.contains("multiple vault notes named 'Alpha'"));
        assert!(rendered.contains("hint: pass a vault-relative path"));
    }

    #[test]
    fn excluded_notes_never_answer_content_links() {
        let directory = synthetic_vault(&[
            ("xlib/Intake.md", "# Intake\n"),
            ("Real.md", "# Real\n"),
        ]);
        let (_root, notes) = vault_notes(&directory);

        // The basename only matches an excluded note: missing, with no
        // suggestion pointing into the excluded directory.
        let error = resolve(&notes, "Real.md", "Intake").expect_err("missing");
        let NoteResolveError::Missing {
            display,
            suggestions,
        } = error
        else {
            panic!("expected missing");
        };
        assert_eq!(display, "Intake");
        assert!(
            !suggestions.iter().any(|stem| stem == "intake"),
            "{suggestions:?}"
        );

        // Even an explicit excluded path is not a content-link target.
        assert!(matches!(
            resolve(&notes, "Real.md", "xlib/Intake"),
            Err(NoteResolveError::Missing { .. })
        ));
    }

    #[test]
    fn same_note_fragments_resolve_to_the_host() {
        let directory = synthetic_vault(&[("Host.md", "# Host\n")]);
        let (root, notes) = vault_notes(&directory);
        let resolved =
            resolve(&notes, "Host.md", "#Some Heading").expect("same note");
        assert!(resolved.same_note);
        assert_eq!(resolved.canonical_path, root.join("Host.md"));
    }

    #[test]
    fn fragment_extents_cover_headings_blocks_and_paragraphs() {
        let text = concat!(
            "---\n",
            "# Fake ^front-id\n",
            "---\n",
            "# Title\n",
            "\n",
            "Intro paragraph line one.\n",
            "Intro paragraph line two. ^intro\n",
            "Intro paragraph line three.\n",
            "\n",
            "## Section ^sec-id\n",
            "body one\n",
            "\n",
            "```\n",
            "# Not a heading ^code-id\n",
            "```\n",
            "\n",
            "%%\n",
            "# Not a heading either ^comment-id\n",
            "%%\n",
            "\n",
            "<!--\n",
            "# Nor this ^html-id\n",
            "-->\n",
            "\n",
            "## Repeated\n",
            "first\n",
            "\n",
            "## Repeated\n",
            "second\n",
            "\n",
            "- [ ] Task one ^t1\n",
            "  - [ ] child\n",
            "\t- tab child\n",
            "- [ ] Task two\n",
        );
        let index = build_fragment_index(text);

        // Frontmatter, code, and every comment form contribute nothing.
        for missing in [
            LinkFragment::Heading("Fake".to_string()),
            LinkFragment::Block("front-id".to_string()),
            LinkFragment::Heading("Not a heading".to_string()),
            LinkFragment::Heading("Not a heading either".to_string()),
            LinkFragment::Heading("Nor this".to_string()),
            LinkFragment::Block("code-id".to_string()),
            LinkFragment::Block("comment-id".to_string()),
            LinkFragment::Block("html-id".to_string()),
            LinkFragment::Heading("Absent".to_string()),
        ] {
            assert_eq!(index.lookup(&missing), FragmentLookup::Missing);
        }

        // Heading embeds run through the next equal-or-shallower heading,
        // including fenced code and comments (the syntax layer projects
        // code and drops comments from that slice).
        let FragmentLookup::Found(section) =
            index.lookup(&LinkFragment::Heading("section".to_string()))
        else {
            panic!("section heading");
        };
        assert_eq!((section.first_line, section.last_line), (10, 23));
        assert!(text[section.byte_start..section.byte_end].contains("body one"));

        // Duplicate headings are ambiguous, never silently first-wins.
        assert_eq!(
            index.lookup(&LinkFragment::Heading("REPEATED".to_string())),
            FragmentLookup::Ambiguous { count: 2 }
        );

        // A heading's own block ID registers alongside its heading text.
        let FragmentLookup::Found(block) =
            index.lookup(&LinkFragment::Block("sec-id".to_string()))
        else {
            panic!("section block id");
        };
        assert_eq!(block.first_line, 10);

        // Paragraph block IDs cover their contiguous paragraph.
        let FragmentLookup::Found(intro) =
            index.lookup(&LinkFragment::Block("intro".to_string()))
        else {
            panic!("intro block id");
        };
        assert_eq!((intro.first_line, intro.last_line), (6, 8));

        // Task block IDs absorb the indented subtree, tabs included.
        let FragmentLookup::Found(task) =
            index.lookup(&LinkFragment::Block("t1".to_string()))
        else {
            panic!("task block id");
        };
        assert_eq!((task.first_line, task.last_line), (31, 33));
        assert!(text[task.byte_start..task.byte_end].contains("tab child"));
    }

    #[test]
    fn fragment_comments_follow_fence_inline_code_and_visible_heading_rules() {
        let text = concat!(
            "```text\n",
            "%%\n",
            "<!--\n",
            "```\n",
            "# Visible after fence\n",
            "# Inline code `%%` title\n",
            "# Inline HTML `<!--` title\n",
            "# Visible %%private%% heading\n",
            "<!--\n",
            "# Hidden by HTML comment\n",
            "-->\n",
            "%%\n",
            "# Hidden by percent comment\n",
            "%%\n",
            "# Visible after comments\n",
        );
        let index = build_fragment_index(text);
        for visible in [
            "Visible after fence",
            "Inline code `%%` title",
            "Inline HTML `<!--` title",
            "Visible  heading",
            "Visible after comments",
        ] {
            assert!(
                matches!(
                    index.lookup(&LinkFragment::Heading(visible.to_string())),
                    FragmentLookup::Found(_)
                ),
                "visible heading was not indexed: {visible}"
            );
        }
        for hidden in ["Hidden by HTML comment", "Hidden by percent comment"] {
            assert!(
                matches!(
                    index.lookup(&LinkFragment::Heading(hidden.to_string())),
                    FragmentLookup::Missing
                ),
                "commented heading was indexed: {hidden}"
            );
        }
    }

    #[test]
    fn attachment_precedence_is_exact_then_note_dir_then_basename() {
        let directory = synthetic_vault(&[
            ("img/shared.png", "root-image"),
            ("notes/img/shared.png", "note-image"),
            ("notes/img/note-only.png", "note-only"),
            ("notes/Host.md", "# Host\n"),
            ("notes/other.md", "# Other\n"),
        ]);
        let (root, _) = vault_notes(&directory);
        let mut index = AttachmentIndex::new(root.clone());

        // Exact vault-relative paths win over nearer same-named files.
        let parsed = parse_attachment_target("notes/img/shared.png");
        let found = index
            .resolve(Path::new("notes/Host.md"), &parsed)
            .expect("exact");
        assert_eq!(
            found.vault_relative_path,
            PathBuf::from("notes/img/shared.png")
        );
        assert_eq!(found.media_kind, AssetMediaKind::Png);

        // Then paths relative to the source note directory.
        let parsed = parse_attachment_target("img/note-only.png");
        let found = index
            .resolve(Path::new("notes/Host.md"), &parsed)
            .expect("note-relative");
        assert_eq!(
            found.vault_relative_path,
            PathBuf::from("notes/img/note-only.png")
        );

        // A bare basename with twins is ambiguous with sorted candidates.
        let parsed = parse_attachment_target("shared.png");
        let error = index
            .resolve(Path::new("other.md"), &parsed)
            .expect_err("ambiguous");
        let AttachmentResolutionError::Ambiguous {
            display,
            candidates,
        } = error
        else {
            panic!("expected ambiguity");
        };
        assert_eq!(display, "shared.png");
        assert_eq!(
            candidates,
            vec![
                "img/shared.png".to_string(),
                "notes/img/shared.png".to_string(),
            ]
        );

        // Unknown names stay missing.
        let parsed = parse_attachment_target("absent.png");
        assert!(matches!(
            index.resolve(Path::new("notes/Host.md"), &parsed),
            Err(AttachmentResolutionError::Missing { .. })
        ));
    }

    #[test]
    fn attachment_width_hints_and_asset_registration() {
        assert_eq!(
            parse_attachment_target("[[img/a.png|400]]"),
            ParsedAttachmentTarget {
                path: "img/a.png".to_string(),
                width_px: Some(400),
            }
        );
        assert_eq!(
            parse_attachment_target("img/a.png|400x300").width_px,
            Some(400)
        );
        // A non-numeric suffix stays part of the path.
        assert_eq!(
            parse_attachment_target("img/a.png|center"),
            ParsedAttachmentTarget {
                path: "img/a.png|center".to_string(),
                width_px: None,
            }
        );

        let directory = synthetic_vault(&[
            ("img/a.PNG", "a"),
            ("img/b.jpg", "b"),
            ("img/c.svg", "c"),
            ("img/d.txt", "d"),
        ]);
        let (root, _) = vault_notes(&directory);
        let mut index = AttachmentIndex::new(root);
        for (name, kind) in [
            ("a.PNG", AssetMediaKind::Png),
            ("b.jpg", AssetMediaKind::Jpeg),
            ("c.svg", AssetMediaKind::Svg),
            ("d.txt", AssetMediaKind::Other),
        ] {
            let parsed = parse_attachment_target(&format!("img/{name}|400"));
            let found = index
                .resolve(Path::new("Host.md"), &parsed)
                .expect("resolve");
            assert_eq!(found.media_kind, kind, "{name}");
            assert_eq!(found.width_px_hint, Some(400));
            let asset = index.register_asset(&found);
            assert_eq!(asset.width_px_hint, Some(400));
            assert_eq!(asset.width_percent, Some(62));
        }

        // Repeats deduplicate to the same scratch-relative asset.
        let parsed = parse_attachment_target("img/a.PNG|200");
        let found = index
            .resolve(Path::new("Host.md"), &parsed)
            .expect("resolve");
        let repeat = index.register_asset(&found);
        let parsed = parse_attachment_target("img/a.PNG|400");
        let found = index
            .resolve(Path::new("Host.md"), &parsed)
            .expect("resolve");
        let first = index.register_asset(&found);
        assert_eq!(repeat.id, first.id);
        assert_eq!(repeat.relative_path, PathBuf::from("assets/1.png"));
    }

    #[test]
    fn attachment_index_builds_lazily_and_caches() {
        // Construction alone performs no I/O, even on a missing root.
        let missing = PathBuf::from("/synthetic-vault-that-is-absent");
        let mut index = AttachmentIndex::new(missing);
        let _ = &mut index;

        let directory = synthetic_vault(&[("img/a.png", "a")]);
        let (root, _) = vault_notes(&directory);
        let mut index = AttachmentIndex::new(root);
        let parsed = parse_attachment_target("a.png");
        let found = index
            .resolve(Path::new("Host.md"), &parsed)
            .expect("basename");
        assert_eq!(found.vault_relative_path, PathBuf::from("img/a.png"));
        // The walk is cached: removing the file cannot lose the entry.
        fs::remove_file(directory.path().join("img/a.png")).expect("remove");
        let again = index
            .resolve(Path::new("Host.md"), &parsed)
            .expect("cached");
        assert_eq!(again.canonical_source_path, found.canonical_source_path);
    }

    #[cfg(unix)]
    #[test]
    fn attachment_symlink_escapes_stay_missing() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().expect("outside dir");
        let secret = outside.path().join("secret.png");
        fs::write(&secret, "secret").expect("write");
        let directory = synthetic_vault(&[("notes/Host.md", "# Host\n")]);
        symlink(&secret, directory.path().join("notes/escape.png"))
            .expect("symlink");
        let (root, _) = vault_notes(&directory);
        let mut index = AttachmentIndex::new(root);
        let parsed = parse_attachment_target("notes/escape.png");
        assert!(matches!(
            index.resolve(Path::new("notes/Host.md"), &parsed),
            Err(AttachmentResolutionError::Missing { .. })
        ));
    }
}
