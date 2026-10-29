//! Vault-wide inbound link rewrite planning for a moved reference bundle.
//!
//! This module only reads the vault. It returns deterministic line edits and
//! the corresponding rewritten contents so the command planner and writer can
//! share exactly the same rewrite logic.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use crate::native::{markdown, task_dependencies};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    /// One-based line number in the original file.
    pub(crate) line: usize,
    pub(crate) before: String,
    pub(crate) after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRewrite {
    /// Vault-relative path, with the host's native separators.
    pub(crate) path: PathBuf,
    pub(crate) edits: Vec<LineEdit>,
    /// Number of individual link targets rewritten in this file.
    pub(crate) rewritten: usize,
    /// Full post-rewrite contents. The writer should write this atomically.
    pub(crate) contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LinkFinding {
    pub(crate) file: PathBuf,
    /// One-based line number.
    pub(crate) line: usize,
    /// Original link target, including any fragment.
    pub(crate) target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AmbiguousBareLink {
    pub(crate) file: PathBuf,
    /// One-based line number.
    pub(crate) line: usize,
    pub(crate) target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LinkRewritePlan {
    pub(crate) files: Vec<FileRewrite>,
    /// Number of distinct inbound link targets that will change.
    pub(crate) rewritten: usize,
    pub(crate) skipped_ambiguous: Vec<AmbiguousBareLink>,
    /// Dangling `#^h-` links intentionally left unchanged when the caller
    /// supplied the old note's known highlight anchors.
    pub(crate) skipped_dangling_highlights: Vec<LinkFinding>,
    /// Inbound links whose fragment is a highlight block ID (`#^h-…`).
    pub(crate) highlight_fragments: Vec<LinkFinding>,
    /// Lines with an `[h:: …]` property that link to the renamed note.
    pub(crate) highlight_follow_ups: Vec<LinkFinding>,
}

/// Which inbound links a rewrite plan is allowed to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkSelector {
    /// Every resolvable mapped path, including companions. Rename uses this.
    All,
    /// Only resolvable note links whose fragment is `#^h-…`, rewritten to a
    /// path-qualified frozen-note target. Note, heading, and `#^s-` links
    /// stay put; an `[h::]` field alone is not permission to move them.
    HighlightFragments,
}

#[derive(Debug)]
pub(crate) enum LinkScanError {
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidUtf8 {
        path: PathBuf,
    },
}

impl fmt::Display for LinkScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadDir { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::ReadFile { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::InvalidUtf8 { path } => {
                write!(formatter, "{} is not valid UTF-8", path.display())
            }
        }
    }
}

impl Error for LinkScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadDir { source, .. } | Self::ReadFile { source, .. } => {
                Some(source)
            }
            Self::InvalidUtf8 { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct MappedPath {
    old: String,
    new: String,
    is_note: bool,
}

#[derive(Debug, Clone, Default)]
struct BundlePaths {
    exact: BTreeMap<String, MappedPath>,
    bare: BTreeMap<String, MappedPath>,
    notes: Vec<MappedPath>,
    bundle_markdown: BTreeSet<String>,
}

impl BundlePaths {
    fn new(bundle: &BTreeMap<PathBuf, PathBuf>) -> Self {
        let mut paths = Self::default();
        for (old_path, new_path) in bundle {
            let old = vault_path(old_path);
            let new = vault_path(new_path);
            let first_component = old.split('/').next().unwrap_or_default();
            let is_note = is_markdown_path(&old)
                && !matches!(first_component, "lib" | "xlib")
                && !old.contains(".textbundle/");
            let mapped = MappedPath {
                old: old.clone(),
                new: new.clone(),
                is_note,
            };
            paths.exact.insert(old.to_ascii_lowercase(), mapped.clone());
            if is_markdown_path(&old) {
                paths.bundle_markdown.insert(old.to_ascii_lowercase());
            }
            if old.to_ascii_lowercase().ends_with(".md") {
                let old_without_extension = old[..old.len() - 3].to_string();
                let new_without_extension =
                    if new.to_ascii_lowercase().ends_with(".md") {
                        new[..new.len() - 3].to_string()
                    } else {
                        new.clone()
                    };
                paths.exact.insert(
                    old_without_extension.to_ascii_lowercase(),
                    MappedPath {
                        old: old_without_extension,
                        new: new_without_extension,
                        is_note,
                    },
                );
            }
            if is_note {
                let old_stem = path_stem(&old).unwrap_or_default().to_string();
                let new_stem = path_stem(&new).unwrap_or_default().to_string();
                paths.bare.insert(
                    old_stem.to_ascii_lowercase(),
                    MappedPath {
                        old: old_stem.clone(),
                        new: new_stem.clone(),
                        is_note: true,
                    },
                );
                paths.bare.insert(
                    format!("{old_stem}.md").to_ascii_lowercase(),
                    MappedPath {
                        old: format!("{old_stem}.md"),
                        new: format!("{new_stem}.md"),
                        is_note: true,
                    },
                );
                paths.notes.push(mapped);
            }
        }
        paths.notes.sort_by(|left, right| left.old.cmp(&right.old));
        paths
    }

    fn map_path(&self, path: &str) -> Option<&MappedPath> {
        self.exact
            .get(&normalize_link_path(path).to_ascii_lowercase())
    }

    fn map_bare(&self, path: &str) -> Option<&MappedPath> {
        self.bare.get(&path.to_ascii_lowercase())
    }

    fn bare_note_candidates(&self, stem: &str) -> Vec<&MappedPath> {
        let stem = bare_stem(stem).to_ascii_lowercase();
        self.notes
            .iter()
            .filter(|note| {
                path_stem(&note.old)
                    .is_some_and(|old| old.eq_ignore_ascii_case(&stem))
            })
            .collect()
    }

    fn bare_is_unique(
        &self,
        old_stem: &str,
        markdown_paths: &[PathBuf],
    ) -> bool {
        !markdown_paths.iter().any(|path| {
            let relative = vault_path(path);
            is_markdown_path(&relative)
                && !self
                    .bundle_markdown
                    .contains(&relative.to_ascii_lowercase())
                && path_stem(&relative)
                    .is_some_and(|stem| stem.eq_ignore_ascii_case(old_stem))
        })
    }
}

/// Scan eligible Markdown files under `vault_root` and build a stable rewrite
/// plan for the supplied old-to-new vault-relative bundle paths.
pub(crate) fn plan_vault_links(
    vault_root: &Path,
    bundle: &BTreeMap<PathBuf, PathBuf>,
) -> Result<LinkRewritePlan, LinkScanError> {
    plan_vault_links_with(vault_root, bundle, LinkSelector::All)
}

/// Scan eligible Markdown files and rewrite only the links `selector` owns.
pub(crate) fn plan_vault_links_with(
    vault_root: &Path,
    bundle: &BTreeMap<PathBuf, PathBuf>,
    selector: LinkSelector,
) -> Result<LinkRewritePlan, LinkScanError> {
    plan_vault_links_with_known_highlights(vault_root, bundle, selector, None)
}

/// Plan rewrites while requiring highlight fragments to resolve in the old
/// note. Dangling highlight links remain unchanged and are returned for a
/// caller-visible warning.
pub(crate) fn plan_vault_links_with_known_highlights(
    vault_root: &Path,
    bundle: &BTreeMap<PathBuf, PathBuf>,
    selector: LinkSelector,
    known_highlights: Option<&BTreeSet<String>>,
) -> Result<LinkRewritePlan, LinkScanError> {
    let mut all_markdown = Vec::new();
    collect_markdown_paths(vault_root, vault_root, &mut all_markdown)?;
    all_markdown.sort();

    let bundle_paths = BundlePaths::new(bundle);
    let mut plan = LinkRewritePlan::default();
    for relative in all_markdown.iter().filter(|path| scan_path(path)) {
        let absolute = vault_root.join(relative);
        let bytes =
            fs::read(&absolute).map_err(|source| LinkScanError::ReadFile {
                path: absolute.clone(),
                source,
            })?;
        let contents = String::from_utf8(bytes).map_err(|_| {
            LinkScanError::InvalidUtf8 {
                path: absolute.clone(),
            }
        })?;
        let result = rewrite_text(
            &contents,
            &bundle_paths,
            &all_markdown,
            relative,
            selector,
            known_highlights,
        );
        if !result.edits.is_empty() {
            plan.files.push(FileRewrite {
                path: relative.clone(),
                edits: result.edits,
                rewritten: result.rewritten,
                contents: result.contents,
            });
        }
        plan.rewritten += result.rewritten;
        plan.skipped_ambiguous.extend(result.skipped_ambiguous);
        plan.skipped_dangling_highlights
            .extend(result.skipped_dangling_highlights);
        plan.highlight_fragments.extend(result.highlight_fragments);
        plan.highlight_follow_ups
            .extend(result.highlight_follow_ups);
    }

    plan.skipped_ambiguous.sort();
    plan.skipped_dangling_highlights.sort();
    plan.highlight_fragments.sort();
    plan.highlight_follow_ups.sort();
    Ok(plan)
}

/// Rewrite one Markdown document in memory with the same rules as the
/// vault-wide planner. Asset and highlight rewrites of a frozen note share
/// this path so overlapping edits land in one contents snapshot.
pub(crate) fn rewrite_contents(
    contents: &str,
    bundle: &BTreeMap<PathBuf, PathBuf>,
    selector: LinkSelector,
    current_file: &Path,
) -> String {
    let bundle_paths = BundlePaths::new(bundle);
    rewrite_text(contents, &bundle_paths, &[], current_file, selector, None)
        .contents
}

#[derive(Debug, Default)]
struct TextRewrite {
    contents: String,
    edits: Vec<LineEdit>,
    rewritten: usize,
    skipped_ambiguous: Vec<AmbiguousBareLink>,
    skipped_dangling_highlights: Vec<LinkFinding>,
    highlight_fragments: Vec<LinkFinding>,
    highlight_follow_ups: Vec<LinkFinding>,
}

fn rewrite_text(
    contents: &str,
    bundle: &BundlePaths,
    markdown_paths: &[PathBuf],
    current_file: &Path,
    selector: LinkSelector,
    known_highlights: Option<&BTreeSet<String>>,
) -> TextRewrite {
    let raw_lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let logical_lines = raw_lines
        .iter()
        .map(|line| line.strip_suffix('\n').unwrap_or(line))
        .collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&logical_lines, 0..logical_lines.len());
    let file = current_file.to_path_buf();
    let mut output = String::with_capacity(contents.len());
    let mut result = TextRewrite::default();

    for (index, raw_line) in raw_lines.iter().enumerate() {
        let line = logical_lines[index];
        if fenced.contains(&index) {
            output.push_str(raw_line);
            continue;
        }

        let line_result = rewrite_line(
            line,
            bundle,
            markdown_paths,
            &file,
            index + 1,
            selector,
            known_highlights,
        );
        output.push_str(&line_result.after);
        if raw_line.ends_with('\n') {
            output.push('\n');
        }
        if line_result.after != line {
            result.edits.push(LineEdit {
                line: index + 1,
                before: line.to_string(),
                after: line_result.after,
            });
        }
        result
            .skipped_ambiguous
            .extend(line_result.skipped_ambiguous);
        result
            .skipped_dangling_highlights
            .extend(line_result.skipped_dangling_highlights);
        result
            .highlight_fragments
            .extend(line_result.highlight_fragments);
        result
            .highlight_follow_ups
            .extend(line_result.highlight_follow_ups);
        result.rewritten += line_result.rewritten;
    }
    result.contents = output;
    result
}

#[derive(Debug, Default)]
struct LineRewrite {
    after: String,
    rewritten: usize,
    skipped_ambiguous: Vec<AmbiguousBareLink>,
    skipped_dangling_highlights: Vec<LinkFinding>,
    highlight_fragments: Vec<LinkFinding>,
    highlight_follow_ups: Vec<LinkFinding>,
}

#[derive(Debug)]
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

#[derive(Debug)]
struct FoundLink {
    path_start: usize,
    path_end: usize,
    original_target: String,
    replacement: Option<String>,
    qualified_replacement: Option<String>,
    ambiguous: bool,
    is_note: bool,
    highlight_fragment: bool,
}

fn rewrite_line(
    line: &str,
    bundle: &BundlePaths,
    markdown_paths: &[PathBuf],
    file: &Path,
    line_number: usize,
    selector: LinkSelector,
    known_highlights: Option<&BTreeSet<String>>,
) -> LineRewrite {
    let mut result = LineRewrite {
        after: line.to_string(),
        ..LineRewrite::default()
    };
    let mut links = Vec::new();
    let code_spans = task_dependencies::inline_code_spans(line);

    for span in task_dependencies::raw_wikilink_spans(line) {
        let inside_start = span.open + 2;
        let inside_end = span.end - 2;
        let inside = &line[inside_start..inside_end];
        let target_end = inside.find('|').unwrap_or(inside.len());
        let target = &inside[..target_end];
        let path_length = target.find('#').unwrap_or(target.len());
        let path = &target[..path_length];
        let suffix = target[path_length..].to_string();
        let path_start = inside_start;
        let path_end = path_start + path.len();
        let (mapped, ambiguous) =
            resolve_wikilink_path(path, bundle, markdown_paths);
        let is_note = mapped.as_ref().is_some_and(|mapped| mapped.is_note)
            || (!path.contains('/')
                && !bundle.bare_note_candidates(path).is_empty());
        let highlight_fragment = is_note && suffix.starts_with("#^h-");
        let qualified_replacement =
            mapped.map(|mapped| qualified_note_path(mapped, path, bundle));
        links.push(FoundLink {
            path_start,
            path_end,
            original_target: target.to_string(),
            replacement: mapped.map(|mapped| mapped.new.clone()),
            qualified_replacement,
            ambiguous,
            is_note,
            highlight_fragment,
        });
    }

    for markdown in markdown_links(line, &code_spans) {
        let raw_path = &line[markdown.path_start..markdown.path_end];
        let (raw_decoded_path, suffix) = split_markdown_path(raw_path);
        let Some(decoded_path) = percent_decode(raw_decoded_path) else {
            continue;
        };
        let (mapped, ambiguous) =
            resolve_wikilink_path(&decoded_path, bundle, markdown_paths);
        let is_note = mapped.is_some_and(|mapped| mapped.is_note)
            || (!decoded_path.contains('/')
                && !bundle.bare_note_candidates(&decoded_path).is_empty());
        if mapped.is_none() && !ambiguous {
            continue;
        }
        let replacement = mapped.map(|mapped| {
            let new_path =
                encode_path_like(mapped.new.as_str(), raw_decoded_path);
            format!("{new_path}{suffix}")
        });
        let qualified_replacement = mapped.map(|mapped| {
            let new_path = encode_path_like(
                &qualified_note_path(mapped, &decoded_path, bundle),
                raw_decoded_path,
            );
            format!("{new_path}{suffix}")
        });
        let highlight_fragment = is_note && suffix.starts_with("#^h-");
        links.push(FoundLink {
            path_start: markdown.path_start,
            path_end: markdown.path_end,
            original_target: raw_path.to_string(),
            replacement,
            qualified_replacement,
            ambiguous,
            is_note,
            highlight_fragment,
        });
    }

    let has_h_property = has_highlight_property(line, &code_spans);
    let mut replacements = Vec::new();
    for link in links {
        if link.ambiguous {
            result.skipped_ambiguous.push(AmbiguousBareLink {
                file: file.to_path_buf(),
                line: line_number,
                target: link.original_target.clone(),
            });
        }
        let finding = LinkFinding {
            file: file.to_path_buf(),
            line: line_number,
            target: link.original_target.clone(),
        };
        let dangling_highlight = selector == LinkSelector::HighlightFragments
            && link.highlight_fragment
            && known_highlights.is_some_and(|known| {
                highlight_anchor(&link.original_target)
                    .is_none_or(|anchor| !known.contains(anchor))
            });
        if link.highlight_fragment && dangling_highlight {
            result.skipped_dangling_highlights.push(finding.clone());
        } else if link.highlight_fragment {
            result.highlight_fragments.push(finding.clone());
        }
        if link.is_note && has_h_property {
            result.highlight_follow_ups.push(LinkFinding {
                file: file.to_path_buf(),
                line: line_number,
                target: link.original_target.clone(),
            });
        }
        let replacement = match selector {
            LinkSelector::All => link.replacement,
            LinkSelector::HighlightFragments
                if link.highlight_fragment && !dangling_highlight =>
            {
                link.qualified_replacement.or(link.replacement)
            }
            LinkSelector::HighlightFragments => None,
        };
        if let Some(replacement) = replacement {
            let old_path = &line[link.path_start..link.path_end];
            if replacement != old_path {
                result.rewritten += 1;
                replacements.push(Replacement {
                    start: link.path_start,
                    end: link.path_end,
                    text: replacement,
                });
            }
        }
    }

    replacements.sort_by_key(|replacement| replacement.start);
    for replacement in replacements.into_iter().rev() {
        result.after.replace_range(
            replacement.start..replacement.end,
            &replacement.text,
        );
    }
    result
}

fn highlight_anchor(target: &str) -> Option<&str> {
    let (_, fragment) = target.rsplit_once("#^")?;
    let anchor = fragment
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '-')
        })
        .next()?;
    anchor.starts_with("h-").then_some(anchor)
}

fn qualified_note_path(
    mapped: &MappedPath,
    original_path: &str,
    bundle: &BundlePaths,
) -> String {
    let full = bundle
        .notes
        .iter()
        .find(|note| {
            let note_stem = path_stem(&note.old).unwrap_or_default();
            let mapped_stem = bare_stem(&mapped.old);
            note.old.eq_ignore_ascii_case(&mapped.old)
                || note_stem.eq_ignore_ascii_case(mapped_stem)
        })
        .map(|note| note.new.as_str())
        .unwrap_or(mapped.new.as_str());
    match_original_markdown_suffix(full, original_path)
}

fn match_original_markdown_suffix(path: &str, original_path: &str) -> String {
    let without_ext = path
        .strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(path);
    if original_path.ends_with(".md") || original_path.ends_with(".MD") {
        format!("{without_ext}.md")
    } else {
        without_ext.to_string()
    }
}

fn resolve_wikilink_path<'a>(
    path: &str,
    bundle: &'a BundlePaths,
    markdown_paths: &[PathBuf],
) -> (Option<&'a MappedPath>, bool) {
    if path.contains('/') || path.contains('\\') {
        return (bundle.map_path(path), false);
    }
    let candidates = bundle.bare_note_candidates(path);
    if candidates.len() != 1 {
        return (None, candidates.len() > 1);
    }
    let mapped = candidates[0];
    let old_stem = path_stem(&mapped.old).unwrap_or_default();
    if !bundle.bare_is_unique(old_stem, markdown_paths) {
        return (None, true);
    }

    // Keep the caller's bare spelling and optional `.md` extension.
    (bundle.map_bare(path), false)
}

#[derive(Debug)]
struct MarkdownLink {
    path_start: usize,
    path_end: usize,
}

fn markdown_links(
    line: &str,
    code_spans: &[std::ops::Range<usize>],
) -> Vec<MarkdownLink> {
    let mut links = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = line[search_from..].find("](") {
        let open = search_from + relative + 1;
        let target_start = open + 1;
        if code_spans
            .iter()
            .any(|span| open < span.end && target_start + 1 > span.start)
        {
            search_from = target_start + 1;
            continue;
        }
        let parsed = if line.as_bytes().get(target_start) == Some(&b'<') {
            line[target_start + 1..]
                .find('>')
                .map(|offset| (target_start + 1, target_start + 1 + offset))
        } else {
            parse_unwrapped_markdown_target(line, target_start)
        };
        let Some((path_start, path_end)) = parsed else {
            search_from = target_start + 1;
            continue;
        };
        if code_spans
            .iter()
            .any(|span| path_start < span.end && path_end > span.start)
        {
            search_from = path_end.max(target_start + 1);
            continue;
        }
        links.push(MarkdownLink {
            path_start,
            path_end,
        });
        search_from = path_end.max(target_start + 1);
    }
    links
}

fn parse_unwrapped_markdown_target(
    line: &str,
    target_start: usize,
) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = target_start;
    let mut depth = 0usize;
    let mut target_end = None;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' if depth == 0 => {
                target_end = Some(index);
                break;
            }
            b')' => {
                depth -= 1;
                index += 1;
            }
            byte if byte.is_ascii_whitespace() && depth == 0 => {
                // An optional Markdown title follows the target. Find the
                // closing parenthesis without including the title in target.
                let mut close = index;
                while close < bytes.len() && bytes[close] != b')' {
                    if bytes[close] == b'\\' {
                        close = (close + 2).min(bytes.len());
                    } else {
                        close += 1;
                    }
                }
                if close == bytes.len() {
                    return None;
                }
                target_end = Some(index);
                index = close;
                break;
            }
            _ => index += 1,
        }
    }
    let target_end = target_end?;
    (target_end > target_start && bytes.get(index) == Some(&b')'))
        .then_some((target_start, target_end))
}

fn split_markdown_path(target: &str) -> (&str, &str) {
    let split = target
        .char_indices()
        .find_map(|(index, character)| {
            matches!(character, '#' | '?').then_some(index)
        })
        .unwrap_or(target.len());
    target.split_at(split)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((hex_value(high)? << 4) | hex_value(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_path_like(path: &str, original: &str) -> String {
    let encoded_slash = original.as_bytes().windows(3).any(|window| {
        window[0] == b'%'
            && window[1].eq_ignore_ascii_case(&b'2')
            && window[2].eq_ignore_ascii_case(&b'f')
    });
    let lower_hex = original
        .as_bytes()
        .windows(3)
        .find(|window| window[0] == b'%')
        .is_some_and(|window| {
            window[1].is_ascii_lowercase() || window[2].is_ascii_lowercase()
        });
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
        {
            encoded.push(char::from(byte));
        } else if byte == b'/' && !encoded_slash {
            encoded.push('/');
        } else {
            let digits = if lower_hex {
                b"0123456789abcdef"
            } else {
                b"0123456789ABCDEF"
            };
            encoded.push('%');
            encoded.push(char::from(digits[(byte >> 4) as usize]));
            encoded.push(char::from(digits[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn has_highlight_property(
    line: &str,
    code_spans: &[std::ops::Range<usize>],
) -> bool {
    let mut from = 0;
    while let Some(relative) = line[from..].find("[h::") {
        let start = from + relative;
        let Some(close) = line[start..].find(']') else {
            from = start + 4;
            continue;
        };
        let end = start + close + 1;
        if !code_spans
            .iter()
            .any(|span| start < span.end && end > span.start)
        {
            return true;
        }
        from = end;
    }
    false
}

fn collect_markdown_paths(
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<(), LinkScanError> {
    let entries =
        fs::read_dir(directory).map_err(|source| LinkScanError::ReadDir {
            path: directory.to_path_buf(),
            source,
        })?;
    let mut entries =
        entries.collect::<Result<Vec<_>, _>>().map_err(|source| {
            LinkScanError::ReadDir {
                path: directory.to_path_buf(),
                source,
            }
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type =
            entry.file_type().map_err(|source| LinkScanError::ReadDir {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name.to_str().is_some_and(|name| name.starts_with('.')) {
                continue;
            }
            collect_markdown_paths(root, &path, output)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            output.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn scan_path(path: &Path) -> bool {
    let relative = vault_path(path);
    if !is_markdown_path(&relative) {
        return false;
    }
    let components = relative.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.starts_with('.'))
    {
        return false;
    }
    for component in &components[..components.len().saturating_sub(1)] {
        if matches!(*component, "_generated" | "_templates" | "_conflicts") {
            return false;
        }
    }
    if components
        .first()
        .is_some_and(|component| matches!(*component, "lib" | "xlib"))
    {
        return false;
    }
    !crate::native::ref_tasks::is_conflict_copy_name(
        components.last().copied().unwrap_or_default(),
    )
}

fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn path_stem(path: &str) -> Option<&str> {
    let filename = path.rsplit('/').next()?;
    filename
        .strip_suffix(".md")
        .or_else(|| filename.strip_suffix(".MD"))
}

fn bare_stem(target: &str) -> &str {
    target
        .strip_suffix(".md")
        .or_else(|| target.strip_suffix(".MD"))
        .unwrap_or(target)
}

fn normalize_link_path(path: &str) -> String {
    let forward = path.replace('\\', "/");
    forward.strip_prefix("./").unwrap_or(&forward).to_string()
}

fn vault_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn bundle(old: &str, new: &str) -> BTreeMap<PathBuf, PathBuf> {
        BTreeMap::from([
            (
                PathBuf::from(format!("ref/blogs/{old}.md")),
                PathBuf::from(format!("ref/blogs/{new}.md")),
            ),
            (
                PathBuf::from(format!("lib/blogs/{old}.pdf")),
                PathBuf::from(format!("lib/blogs/{new}.pdf")),
            ),
            (
                PathBuf::from(format!("lib/blogs/{old}.md")),
                PathBuf::from(format!("lib/blogs/{new}.md")),
            ),
            (
                PathBuf::from(format!("lib/blogs/{old}.textbundle/text.md")),
                PathBuf::from(format!("lib/blogs/{new}.textbundle/text.md")),
            ),
            (
                PathBuf::from(format!("lib/blogs/{old}.mp3")),
                PathBuf::from(format!("lib/blogs/{new}.mp3")),
            ),
        ])
    }

    fn rewrite(
        input: &str,
        moves: &BTreeMap<PathBuf, PathBuf>,
        notes: &[&str],
    ) -> TextRewrite {
        rewrite_with(input, moves, notes, LinkSelector::All)
    }

    fn rewrite_with(
        input: &str,
        moves: &BTreeMap<PathBuf, PathBuf>,
        notes: &[&str],
        selector: LinkSelector,
    ) -> TextRewrite {
        let bundle = BundlePaths::new(moves);
        let note_paths = notes.iter().map(PathBuf::from).collect::<Vec<_>>();
        rewrite_text(
            input,
            &bundle,
            &note_paths,
            Path::new("sase.md"),
            selector,
            None,
        )
    }

    #[test]
    fn rewrites_wikilinks_embeds_fragments_aliases_and_companion_links() {
        let moves = bundle("old", "new");
        let input = concat!(
            "[[ref/blogs/old]] [[ref/blogs/old.md#Section|read]] ",
            "![[ref/blogs/old#^task|embed]] ",
            "[[lib/blogs/old.pdf#^ref]] ![[lib/blogs/old.pdf]] ",
            "[[lib/blogs/old.mp3]] [[lib/blogs/old.textbundle/text.md]]\n"
        );
        let result = rewrite(input, &moves, &["ref/blogs/old.md"]);
        assert_eq!(
            result.contents,
            concat!(
                "[[ref/blogs/new]] [[ref/blogs/new.md#Section|read]] ",
                "![[ref/blogs/new#^task|embed]] ",
                "[[lib/blogs/new.pdf#^ref]] ![[lib/blogs/new.pdf]] ",
                "[[lib/blogs/new.mp3]] [[lib/blogs/new.textbundle/text.md]]\n"
            )
        );
    }

    #[test]
    fn rewrites_unique_bare_note_links_and_reports_ambiguous_ones() {
        let moves = bundle("old", "new");
        let unique =
            rewrite("[[old]] [[old.md|alias]]", &moves, &["ref/blogs/old.md"]);
        assert_eq!(unique.contents, "[[new]] [[new.md|alias]]");

        let ambiguous = rewrite(
            "[[old#^h-one]]",
            &moves,
            &["ref/blogs/old.md", "Archive/old.md"],
        );
        assert_eq!(ambiguous.contents, "[[old#^h-one]]");
        assert_eq!(ambiguous.skipped_ambiguous.len(), 1);
        assert_eq!(ambiguous.highlight_fragments.len(), 1);
    }

    #[test]
    fn rewrites_section_block_and_heading_links_with_the_note() {
        let moves = bundle("old", "new");
        let result = rewrite(
            "[[old#^s-memory]] [[old#Memory]]",
            &moves,
            &["ref/blogs/old.md"],
        );
        assert_eq!(result.contents, "[[new#^s-memory]] [[new#Memory]]");
    }

    #[test]
    fn rewrites_unique_bare_markdown_note_links_and_reports_ambiguous_ones() {
        let moves = bundle("old", "new");
        let unique = rewrite(
            "[note](old.md#^h-one) [note](old)",
            &moves,
            &["ref/blogs/old.md"],
        );
        assert_eq!(unique.contents, "[note](new.md#^h-one) [note](new)");
        assert_eq!(unique.highlight_fragments.len(), 1);

        let ambiguous = rewrite(
            "[note](old.md#^h-one)",
            &moves,
            &["ref/blogs/old.md", "Archive/old.md"],
        );
        assert_eq!(ambiguous.contents, "[note](old.md#^h-one)");
        assert_eq!(ambiguous.skipped_ambiguous.len(), 1);
        assert_eq!(ambiguous.highlight_fragments.len(), 1);
    }

    #[test]
    fn preserves_highlight_follow_up_findings_without_touching_hash_identity_text(
    ) {
        let moves =
            bundle("442a6bf555914893e9891c11519de94f", "karpathy_llm_wiki");
        let hash = "442a6bf555914893e9891c11519de94f";
        let input = format!(
            "source_url: https://gist.github.com/karpathy/{hash}\n[h:: note] [[ref/blogs/{hash}.md#^h-title]]\n"
        );
        let result =
            rewrite(&input, &moves, &[&format!("ref/blogs/{hash}.md")]);
        assert!(result.contents.contains(&format!(
            "source_url: https://gist.github.com/karpathy/{hash}"
        )));
        assert!(result
            .contents
            .contains("[[ref/blogs/karpathy_llm_wiki.md#^h-title]]"));
        assert_eq!(result.highlight_fragments.len(), 1);
        assert_eq!(result.highlight_follow_ups.len(), 1);
    }

    #[test]
    fn skips_inline_and_fenced_code_and_scans_frontmatter_done_and_daily_notes()
    {
        let temp = tempfile::tempdir().expect("vault");
        fs::create_dir_all(temp.path().join("done")).unwrap();
        fs::create_dir_all(temp.path().join("daily")).unwrap();
        fs::create_dir_all(temp.path().join(".hidden")).unwrap();
        fs::create_dir_all(temp.path().join("_generated")).unwrap();
        fs::write(
            temp.path().join("frontmatter.md"),
            "---\nparent: \"[[ref/blogs/old]]\"\n---\ninline `[[ref/blogs/old]]`\n```md\n[[ref/blogs/old]]\n```\n",
        )
        .unwrap();
        fs::write(temp.path().join("done/old-work.md"), "[[ref/blogs/old]]\n")
            .unwrap();
        fs::write(
            temp.path().join("daily/2026-10-15.md"),
            "[[ref/blogs/old]]\n",
        )
        .unwrap();
        fs::write(temp.path().join(".hidden/hidden.md"), "[[ref/blogs/old]]\n")
            .unwrap();
        fs::write(
            temp.path().join("_generated/generated.md"),
            "[[ref/blogs/old]]\n",
        )
        .unwrap();
        let moves = bundle("old", "new");

        let plan = plan_vault_links(temp.path(), &moves).unwrap();
        let changed = plan
            .files
            .iter()
            .map(|file| vault_path(&file.path))
            .collect::<Vec<_>>();
        assert_eq!(
            changed,
            ["daily/2026-10-15.md", "done/old-work.md", "frontmatter.md"]
        );
        let frontmatter = plan
            .files
            .iter()
            .find(|file| file.path == Path::new("frontmatter.md"))
            .unwrap();
        assert!(frontmatter
            .contents
            .contains("parent: \"[[ref/blogs/new]]\""));
        assert!(frontmatter.contents.contains("`[[ref/blogs/old]]`"));
        assert!(frontmatter
            .contents
            .contains("```md\n[[ref/blogs/old]]\n```"));
    }

    #[test]
    fn handles_dotted_stems_and_percent_encoded_markdown_paths() {
        let moves = bundle("a.b", "c.d");
        let input =
            "[pdf](lib%2Fblogs%2Fa.b.pdf) [note](<ref/blogs/a.b.md#Title>)\n";
        let result = rewrite(input, &moves, &["ref/blogs/a.b.md"]);
        assert_eq!(
            result.contents,
            "[pdf](lib%2Fblogs%2Fc.d.pdf) [note](<ref/blogs/c.d.md#Title>)\n"
        );
        assert_eq!(
            rewrite(&result.contents, &moves, &["ref/blogs/a.b.md"])
                .edits
                .len(),
            0
        );
    }

    #[test]
    fn markdown_highlight_fragments_are_reported_and_code_targets_are_untouched(
    ) {
        let moves = bundle("old", "new");
        let input =
            "[highlight](ref/blogs/old.md#^h-abc) `[code](ref/blogs/old.md)`\n";
        let result = rewrite(input, &moves, &["ref/blogs/old.md"]);
        assert_eq!(
            result.contents,
            "[highlight](ref/blogs/new.md#^h-abc) `[code](ref/blogs/old.md)`\n"
        );
        assert_eq!(result.highlight_fragments.len(), 1);
    }

    #[test]
    fn highlight_fragments_rewrite_only_h_anchors_and_qualify_bare_targets() {
        let moves = BTreeMap::from([(
            PathBuf::from("ref/blogs/old.md"),
            PathBuf::from("ref_history/blogs/old__2026-10-25.md"),
        )]);
        let input = concat!(
            "[[old#^h-one]] [[ref/blogs/old#^h-two|alias]] ",
            "[[old]] [[old#Heading]] [[old#^s-memory]] ",
            "[md](old.md#^h-three) `[code](old.md#^h-one)`\n",
            "[h:: keep] [[ref/blogs/old]]\n"
        );
        let result = rewrite_with(
            input,
            &moves,
            &["ref/blogs/old.md"],
            LinkSelector::HighlightFragments,
        );
        assert_eq!(
            result.contents,
            concat!(
                "[[ref_history/blogs/old__2026-10-25#^h-one]] ",
                "[[ref_history/blogs/old__2026-10-25#^h-two|alias]] ",
                "[[old]] [[old#Heading]] [[old#^s-memory]] ",
                "[md](ref_history/blogs/old__2026-10-25.md#^h-three) ",
                "`[code](old.md#^h-one)`\n",
                "[h:: keep] [[ref/blogs/old]]\n"
            )
        );
        assert_eq!(result.highlight_fragments.len(), 3);
        assert_eq!(result.highlight_follow_ups.len(), 1);
        assert_eq!(result.rewritten, 3);
    }

    #[test]
    fn known_highlight_anchors_leave_dangling_links_unchanged() {
        let moves = BTreeMap::from([(
            PathBuf::from("ref/blogs/old.md"),
            PathBuf::from("ref_history/blogs/old__2026-10-25.md"),
        )]);
        let bundle = BundlePaths::new(&moves);
        let notes = [PathBuf::from("ref/blogs/old.md")];
        let known = BTreeSet::from(["h-present".to_string()]);
        let input = concat!(
            "[[ref/blogs/old#^h-present]] ",
            "[[ref/blogs/old#^h-missing]]\n"
        );

        let result = rewrite_text(
            input,
            &bundle,
            &notes,
            Path::new("sase.md"),
            LinkSelector::HighlightFragments,
            Some(&known),
        );
        assert_eq!(
            result.contents,
            concat!(
                "[[ref_history/blogs/old__2026-10-25#^h-present]] ",
                "[[ref/blogs/old#^h-missing]]\n"
            )
        );
        assert_eq!(result.rewritten, 1);
        assert_eq!(result.highlight_fragments.len(), 1);
        assert_eq!(
            result.highlight_fragments[0].target,
            "ref/blogs/old#^h-present"
        );
        assert_eq!(result.skipped_dangling_highlights.len(), 1);
        assert_eq!(
            result.skipped_dangling_highlights[0].target,
            "ref/blogs/old#^h-missing"
        );
    }

    #[test]
    fn highlight_fragments_leave_ambiguous_bare_targets_unresolved() {
        let moves = BTreeMap::from([(
            PathBuf::from("ref/blogs/old.md"),
            PathBuf::from("ref_history/blogs/old__2026-10-25.md"),
        )]);
        let result = rewrite_with(
            "[[old#^h-one]]",
            &moves,
            &["ref/blogs/old.md", "Archive/old.md"],
            LinkSelector::HighlightFragments,
        );
        assert_eq!(result.contents, "[[old#^h-one]]");
        assert_eq!(result.skipped_ambiguous.len(), 1);
        assert_eq!(result.highlight_fragments.len(), 1);
        assert_eq!(result.rewritten, 0);
    }

    /// Preservation contract (PT13): `bob ref rename`'s alias overwrite
    /// rewrites the link target while keeping a distinctive authored
    /// task theme field byte-identical.
    #[test]
    fn task_theme_field_survives_ref_rename_alias_overwrite() {
        let moves = bundle("old", "new");
        let input =
            "- [ ] #task Read [[ref/blogs/old.md|alias]] (theme::  WRITE) ^t1\n";
        let result = rewrite(input, &moves, &["ref/blogs/old.md"]);
        assert_eq!(
            result.contents,
            "- [ ] #task Read [[ref/blogs/new.md|alias]] (theme::  WRITE) ^t1\n"
        );
    }

    /// Preservation contract (R11): `bob ref rename`'s alias overwrite
    /// rewrites the link target while keeping a distinctive authored
    /// task root field byte-identical.
    #[test]
    fn task_root_field_survives_ref_rename_alias_overwrite() {
        let moves = bundle("old", "new");
        let input =
            "- [ ] #task Read [[ref/blogs/old.md|alias]] (root::  sase) ^t1\n";
        let result = rewrite(input, &moves, &["ref/blogs/old.md"]);
        assert_eq!(
            result.contents,
            "- [ ] #task Read [[ref/blogs/new.md|alias]] (root::  sase) ^t1\n"
        );
    }
}
