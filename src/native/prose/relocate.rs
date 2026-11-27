//! Link relocation for prose-section extraction and inlining.
//!
//! This module plans text changes only. The conversion planner owns the
//! transaction and may combine these after-images with its other rewrites.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use super::{
    links::{self, LinkSpan},
    scan::{self, ClassifiedLine},
    section_model::{self, ProseSection},
    sections::{
        heading_identity, heading_plain_identity, normalize_identity,
        plan_headings, PlannedHeading,
    },
    vault,
};
use crate::native::vault_links::{LinkResolution, VaultLinkResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Extract,
    Inline,
}

/// The old heading tree and the two locations its contents move between.
#[derive(Debug, Clone)]
pub(crate) struct RelocationMap {
    owner: PathBuf,
    child: PathBuf,
    headings: Vec<PlannedHeading>,
    moved_headings: BTreeSet<usize>,
    moved_blocks: BTreeSet<String>,
    staying_blocks: BTreeSet<String>,
    kept_heading: usize,
    body_range: std::ops::Range<usize>,
}

impl RelocationMap {
    /// Build the extraction map from a section already accepted by the
    /// section model. The new note is placed alongside its owner.
    pub(crate) fn new(section: &ProseSection<'_>, new_stem: &str) -> Self {
        let document = scan::classify(section.text);
        let headings = plan_headings(&document.lines, true).headings;
        let kept_heading = headings
            .iter()
            .position(|heading| heading.line + 1 == section.heading_line)
            .unwrap_or(0);
        let moved_lines = section
            .moved_headings
            .iter()
            .map(|heading| heading.line)
            .collect::<BTreeSet<_>>();
        let moved_headings = headings
            .iter()
            .enumerate()
            .filter(|(_, heading)| moved_lines.contains(&(heading.line + 1)))
            .map(|(index, _)| index)
            .collect();
        let moved_blocks = section
            .moved_block_ids
            .iter()
            .map(|block| block.id.clone())
            .collect();
        let all_blocks = section_model::collect_block_ids(&document.lines);
        let staying_blocks = all_blocks
            .iter()
            .filter(|block| !section.body_range.contains(&(block.line - 1)))
            .map(|block| block.id.clone())
            .collect();
        let mut child = section
            .path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        child.push(format!("{new_stem}.md"));
        Self {
            owner: section.path.clone(),
            child,
            headings,
            moved_headings,
            moved_blocks,
            staying_blocks,
            kept_heading,
            body_range: section.body_range.clone(),
        }
    }

    /// Build the inverse map from the parent's selected stub section and the
    /// child note that will be inlined there.
    pub(crate) fn for_inline(
        section: &ProseSection<'_>,
        child_stem: &str,
        child_text: &str,
    ) -> Self {
        let mut map = Self::new(section, child_stem);
        let owner_doc = scan::classify(section.text);
        let child_doc = scan::classify(child_text);
        let child_numbering = plan_headings(&child_doc.lines, false);
        let mut child_lines = Vec::new();
        let mut visible_lines = BTreeMap::new();
        for line in &child_doc.lines {
            if line.region == scan::Region::Frontmatter {
                continue;
            }
            visible_lines.insert(line.index, child_lines.len());
            child_lines.push(line.content.to_owned());
        }
        let promoted = section.heading_level.saturating_sub(1);
        for heading in &child_numbering.headings {
            let Some(line_index) = visible_lines.get(&heading.line).copied()
            else {
                continue;
            };
            let level = heading.original_level.saturating_add(promoted);
            if (1..=6).contains(&level) {
                child_lines[line_index] =
                    format!("{} {}", "#".repeat(level), heading.title);
            }
            if let Some(underline) = heading.underline
                && let Some(underline_index) = visible_lines.get(&underline)
            {
                child_lines[*underline_index].clear();
            }
        }

        let mut merged_lines = owner_doc
            .lines
            .iter()
            .map(|line| line.content.to_owned())
            .collect::<Vec<_>>();
        let insert_at = section.body_range.start;
        let child_line_count = child_lines.len();
        merged_lines.splice(section.body_range.clone(), child_lines);
        let merged = merged_lines.join("\n");
        let merged_doc = scan::classify(&merged);
        map.headings = plan_headings(&merged_doc.lines, true).headings;
        let moved_range = insert_at..insert_at + child_line_count;
        map.moved_headings = map
            .headings
            .iter()
            .enumerate()
            .filter(|(_, heading)| moved_range.contains(&heading.line))
            .map(|(index, _)| index)
            .collect();
        map.kept_heading = map
            .headings
            .iter()
            .position(|heading| heading.line + 1 == section.heading_line)
            .unwrap_or(0);
        let all_blocks = section_model::collect_block_ids(&owner_doc.lines);
        map.staying_blocks = all_blocks
            .iter()
            .filter(|block| !section.body_range.contains(&(block.line - 1)))
            .map(|block| block.id.clone())
            .collect();
        map.moved_blocks = section_model::collect_block_ids(&child_doc.lines)
            .into_iter()
            .map(|block| block.id)
            .collect();
        map
    }

    pub(crate) fn owner(&self) -> &Path {
        &self.owner
    }

    pub(crate) fn child(&self) -> &Path {
        &self.child
    }

    pub(crate) fn rewrite_note(
        &self,
        root: &Path,
        current_path: &Path,
        contents: &str,
        direction: Direction,
        skip_lines: &[usize],
    ) -> Result<NoteRewrite, RelocationError> {
        let resolver = VaultLinkResolver::new(root);
        // A standalone rewrite may not have a filesystem snapshot. Add the
        // mapped notes to the resolver's in-memory index for exact and bare
        // link resolution.
        let resolver = ResolverWithMappedNotes::new(resolver, self);
        self.rewrite_with_resolver(
            current_path,
            contents,
            direction,
            skip_lines,
            &resolver,
        )
    }

    fn rewrite_with_resolver(
        &self,
        current_path: &Path,
        contents: &str,
        direction: Direction,
        skip_lines: &[usize],
        resolver: &ResolverWithMappedNotes<'_>,
    ) -> Result<NoteRewrite, RelocationError> {
        let document = scan::classify(contents);
        let skipped = skip_lines.iter().copied().collect::<BTreeSet<_>>();
        let mut output = String::with_capacity(contents.len());
        let mut changes = Vec::new();
        if document.bom {
            output.push('\u{feff}');
        }
        for line in &document.lines {
            let mut rewritten = line.content.to_owned();
            if !links::protects_links(line.region)
                && !skipped.contains(&(line.index + 1))
            {
                rewritten = self.rewrite_line(
                    line,
                    current_path,
                    direction,
                    resolver,
                    &mut changes,
                )?;
            }
            output.push_str(&rewritten);
            output.push_str(line.ending);
        }
        Ok(NoteRewrite {
            path: current_path.to_path_buf(),
            links: changes,
            contents: output,
        })
    }

    fn rewrite_line(
        &self,
        line: &ClassifiedLine<'_>,
        current_path: &Path,
        direction: Direction,
        resolver: &ResolverWithMappedNotes<'_>,
        changes: &mut Vec<LinkRewrite>,
    ) -> Result<String, RelocationError> {
        let spans = links::link_spans(line.content);
        let mut updated = line.content.to_owned();
        for span in spans.into_iter().rev() {
            let before = &line.content[span.target_start..span.target_end];
            let Some(after) = self.rewrite_target(
                before,
                span,
                line.index,
                current_path,
                direction,
                resolver,
            )?
            else {
                continue;
            };
            if after == before {
                continue;
            }
            updated.replace_range(span.target_start..span.target_end, &after);
            changes.push(LinkRewrite {
                path: current_path.to_path_buf(),
                line: line.index + 1,
                before: before.to_string(),
                after,
            });
        }
        Ok(updated)
    }

    fn rewrite_target(
        &self,
        raw: &str,
        span: LinkSpan,
        line: usize,
        current_path: &Path,
        direction: Direction,
        resolver: &ResolverWithMappedNotes<'_>,
    ) -> Result<Option<String>, RelocationError> {
        let alias_at = span.wikilink.then(|| raw.find('|')).flatten();
        let destination = alias_at.map_or(raw, |index| &raw[..index]);
        let hash_at = destination.find('#');
        let note = hash_at.map_or(destination, |index| &destination[..index]);
        let fragment = hash_at.map(|index| &destination[index + 1..]);
        let target_path = resolver.resolve(current_path, note, span.wikilink);

        let source_after = self.source_after(current_path, line, direction);
        let mut target_after = target_path.clone();
        let mut fragment_after = fragment.map(str::to_string);
        let mut should_rewrite = false;

        match direction {
            Direction::Extract if same_path(&target_path, &self.owner) => {
                match fragment {
                    Some(fragment) if fragment.starts_with('^') => {
                        let id = &fragment[1..];
                        if self.moved_blocks.contains(id) {
                            self.ensure_block_unambiguous(
                                id,
                                line,
                                current_path,
                                raw,
                            )?;
                            target_after = self.child.clone();
                            fragment_after = Some(format!("^{id}"));
                            should_rewrite = true;
                        } else if self.staying_blocks.contains(id)
                            && same_path(&source_after, &self.child)
                        {
                            target_after = self.owner.clone();
                            fragment_after = Some(format!("^{id}"));
                            should_rewrite = true;
                        }
                    }
                    Some(fragment) => {
                        let Some((index, moved)) = self.resolve_owner_heading(
                            fragment,
                            line,
                            current_path,
                            raw,
                        )?
                        else {
                            return Ok(None);
                        };
                        if moved {
                            target_after = self.child.clone();
                            fragment_after = Some(self.canonical_fragment(
                                index,
                                &self.moved_headings,
                                span.wikilink,
                                fragment,
                            ));
                            should_rewrite = true;
                        } else if same_path(&source_after, &self.child) {
                            fragment_after = Some(self.canonical_fragment(
                                index,
                                &(0..self.headings.len()).collect(),
                                span.wikilink,
                                fragment,
                            ));
                            should_rewrite = true;
                        }
                    }
                    None => {}
                }
            }
            Direction::Inline if same_path(&target_path, &self.child) => {
                match fragment {
                    Some(fragment) if fragment.starts_with('^') => {
                        let id = &fragment[1..];
                        if self.moved_blocks.contains(id) {
                            self.ensure_block_unambiguous(
                                id,
                                line,
                                current_path,
                                raw,
                            )?;
                            target_after = self.owner.clone();
                            fragment_after = Some(format!("^{id}"));
                            should_rewrite = true;
                        } else {
                            target_after = self.owner.clone();
                            should_rewrite = true;
                        }
                    }
                    Some(fragment) => {
                        if let Some(index) =
                            self.resolve_moved_heading(fragment)
                        {
                            target_after = self.owner.clone();
                            fragment_after = Some(self.canonical_fragment(
                                index,
                                &(0..self.headings.len()).collect(),
                                span.wikilink,
                                fragment,
                            ));
                        } else {
                            // Keep a pre-existing dangling fragment dangling,
                            // but make its path survive the note's trashing.
                            target_after = self.owner.clone();
                            fragment_after = Some(self.prepend_kept_fragment(
                                fragment,
                                span.wikilink,
                            ));
                        }
                        should_rewrite = true;
                    }
                    None => {
                        target_after = self.owner.clone();
                        fragment_after = Some(self.canonical_fragment(
                            self.kept_heading,
                            &(0..self.headings.len()).collect(),
                            span.wikilink,
                            "",
                        ));
                        should_rewrite = true;
                    }
                }
            }
            Direction::Inline
                if same_path(&target_path, &self.owner)
                    && same_path(current_path, &self.child)
                    && fragment.is_some() =>
            {
                let fragment = fragment.unwrap_or_default();
                if fragment.starts_with('^') {
                    should_rewrite = true;
                } else if let Some((index, _)) = self.resolve_owner_heading(
                    fragment,
                    line,
                    current_path,
                    raw,
                )? {
                    fragment_after = Some(self.canonical_fragment(
                        index,
                        &(0..self.headings.len()).collect(),
                        span.wikilink,
                        fragment,
                    ));
                    should_rewrite = true;
                }
            }
            _ => {}
        }

        if !should_rewrite {
            return Ok(None);
        }
        let source_after = self.source_after(current_path, line, direction);
        let rendered_note = if same_path(&source_after, &target_after) {
            String::new()
        } else {
            self.render_note_path(
                note,
                &target_after,
                current_path,
                span.wikilink,
            )
        };
        let mut rewritten = rendered_note;
        if let Some(fragment) = fragment_after {
            rewritten.push('#');
            rewritten.push_str(&fragment);
        }
        if let Some(alias_at) = alias_at {
            rewritten.push_str(&raw[alias_at..]);
        }
        Ok(Some(rewritten))
    }

    fn source_after(
        &self,
        current_path: &Path,
        line: usize,
        direction: Direction,
    ) -> PathBuf {
        match direction {
            Direction::Extract
                if same_path(current_path, &self.owner)
                    && self.body_range.contains(&line) =>
            {
                self.child.clone()
            }
            Direction::Inline if same_path(current_path, &self.child) => {
                self.owner.clone()
            }
            _ => current_path.to_path_buf(),
        }
    }

    fn resolve_owner_heading(
        &self,
        fragment: &str,
        line: usize,
        current_path: &Path,
        raw: &str,
    ) -> Result<Option<(usize, bool)>, RelocationError> {
        let matches = resolve_fragment(fragment, &self.headings, None);
        if matches.is_empty() {
            return Ok(None);
        }
        let moved = matches
            .iter()
            .map(|index| self.moved_headings.contains(index))
            .collect::<BTreeSet<_>>();
        if moved.len() > 1 {
            return Err(RelocationError::AmbiguousLink {
                path: current_path.to_path_buf(),
                line: line + 1,
                target: raw.to_string(),
            });
        }
        let index = matches[0];
        Ok(Some((index, self.moved_headings.contains(&index))))
    }

    fn resolve_moved_heading(&self, fragment: &str) -> Option<usize> {
        resolve_fragment(fragment, &self.headings, Some(&self.moved_headings))
            .into_iter()
            .next()
    }

    fn ensure_block_unambiguous(
        &self,
        id: &str,
        line: usize,
        current_path: &Path,
        raw: &str,
    ) -> Result<(), RelocationError> {
        if self.staying_blocks.contains(id) {
            return Err(RelocationError::AmbiguousLink {
                path: current_path.to_path_buf(),
                line: line + 1,
                target: raw.to_string(),
            });
        }
        Ok(())
    }

    fn canonical_fragment(
        &self,
        index: usize,
        allowed: &BTreeSet<usize>,
        wikilink: bool,
        old_fragment: &str,
    ) -> String {
        let chain = self.heading_chain(index, allowed);
        for start in 0..chain.len() {
            let candidate = chain[start..]
                .iter()
                .map(|heading| heading.title.as_str())
                .collect::<Vec<_>>()
                .join("#");
            let matches =
                resolve_fragment(&candidate, &self.headings, Some(allowed));
            if matches.as_slice() == [index] {
                return encode_fragment(&candidate, wikilink, old_fragment);
            }
        }
        let candidate = chain
            .iter()
            .map(|heading| heading.title.as_str())
            .collect::<Vec<_>>()
            .join("#");
        encode_fragment(&candidate, wikilink, old_fragment)
    }

    fn heading_chain<'a>(
        &'a self,
        index: usize,
        allowed: &BTreeSet<usize>,
    ) -> Vec<&'a PlannedHeading> {
        let mut chain = vec![index];
        let mut parent_level = self.headings[index].new_level;
        for earlier in (0..index).rev() {
            if self.headings[earlier].new_level < parent_level {
                if allowed.contains(&earlier) {
                    chain.push(earlier);
                }
                parent_level = self.headings[earlier].new_level;
            }
        }
        chain.reverse();
        chain
            .into_iter()
            .filter(|index| allowed.contains(index))
            .map(|index| &self.headings[index])
            .collect()
    }

    fn prepend_kept_fragment(&self, fragment: &str, wikilink: bool) -> String {
        let kept = self.canonical_fragment(
            self.kept_heading,
            &(0..self.headings.len()).collect(),
            wikilink,
            fragment,
        );
        format!("{kept}#{fragment}")
    }

    fn render_note_path(
        &self,
        original_note: &str,
        destination: &Path,
        current_path: &Path,
        wikilink: bool,
    ) -> String {
        let suffix = markdown_suffix(original_note);
        let path_form = original_note.contains('/')
            || original_note.contains('\\')
            || original_note.starts_with('.');
        let absolute = original_note.starts_with('/');
        let destination_without_extension = destination
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(destination.file_stem().unwrap_or_default());
        let mut rendered = if !path_form {
            destination
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else if absolute {
            format!("/{}", slash_path(&destination_without_extension))
        } else {
            relative_path(
                current_path.parent().unwrap_or_else(|| Path::new("")),
                &destination_without_extension,
            )
        };
        if !suffix.is_empty() {
            rendered.push_str(suffix);
        }
        if !wikilink || original_note.contains('%') {
            percent_encode_path(&rendered)
        } else {
            rendered
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRewrite {
    pub(crate) path: PathBuf,
    /// One-based physical line in the original file.
    pub(crate) line: usize,
    /// Link destination before rewriting, including any fragment and alias.
    pub(crate) before: String,
    /// Link destination after rewriting, including any fragment and alias.
    pub(crate) after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteRewrite {
    pub(crate) path: PathBuf,
    pub(crate) links: Vec<LinkRewrite>,
    pub(crate) contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RewrittenFile {
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VaultRelocationPlan {
    pub(crate) links: Vec<LinkRewrite>,
    pub(crate) files: Vec<RewrittenFile>,
}

/// Scan eligible Markdown notes and return after-images; no file is written.
/// `skip_line` lets inline omit the parent section's link stub, which its
/// caller removes as part of the same conversion transaction.
pub(crate) fn plan_vault(
    root: &Path,
    map: &RelocationMap,
    direction: Direction,
    skip_line: Option<usize>,
) -> Result<VaultRelocationPlan, RelocationError> {
    let paths = vault::list_markdown(root)
        .map_err(|error| RelocationError::Vault(error.to_string()))?;
    let resolver =
        ResolverWithMappedNotes::new(VaultLinkResolver::new(root), map);
    let needle = match direction {
        Direction::Extract => path_stem(&map.owner),
        Direction::Inline => path_stem(&map.child),
    }
    .unwrap_or_default()
    .to_lowercase();
    let owner = normalize_path(&map.owner);
    let child = normalize_path(&map.child);
    let mut plan = VaultRelocationPlan::default();
    for path in paths {
        let normalized = normalize_path(&path);
        let is_mapped = normalized == owner || normalized == child;
        let bytes = fs::read(root.join(&path)).map_err(|source| {
            RelocationError::ReadFile {
                path: path.clone(),
                source,
            }
        })?;
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if !is_mapped && !contains_casefolded_stem(&text, &needle) {
            continue;
        }
        let skipped = if direction == Direction::Inline && normalized == owner {
            skip_line.into_iter().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let rewritten = map.rewrite_with_resolver(
            &path, &text, direction, &skipped, &resolver,
        )?;
        if !rewritten.links.is_empty() {
            plan.links.extend(rewritten.links);
            plan.files.push(RewrittenFile {
                path,
                contents: rewritten.contents,
            });
        }
    }
    plan.links.sort_by(|left, right| {
        (&left.path, left.line, &left.before).cmp(&(
            &right.path,
            right.line,
            &right.before,
        ))
    });
    plan.files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(plan)
}

struct ResolverWithMappedNotes<'a> {
    resolver: VaultLinkResolver,
    index: crate::native::vault_links::NoteIndex,
    map: &'a RelocationMap,
}

impl<'a> ResolverWithMappedNotes<'a> {
    fn new(resolver: VaultLinkResolver, map: &'a RelocationMap) -> Self {
        let index = resolver
            .note_index()
            .with_additional([map.owner.clone(), map.child.clone()]);
        Self {
            resolver,
            index,
            map,
        }
    }

    fn resolve(
        &self,
        current_path: &Path,
        note: &str,
        wikilink: bool,
    ) -> PathBuf {
        if note.is_empty() {
            return current_path.to_path_buf();
        }
        let decoded = percent_decode(note).replace('\\', "/");
        let normalized = lookup_target(current_path, &decoded, wikilink);
        if same_path(&normalized, &self.map.owner)
            || same_path(&normalized, &self.map.child)
        {
            return normalized;
        }
        let target = slash_path(&normalized);
        match self.resolver.resolve(current_path, &target) {
            LinkResolution::Found(path) => path,
            LinkResolution::Ambiguous | LinkResolution::Missing => self
                .index
                .resolve(Some(current_path), &target)
                .unwrap_or(normalized),
        }
    }
}

#[derive(Debug)]
pub(crate) enum RelocationError {
    AmbiguousLink {
        path: PathBuf,
        line: usize,
        target: String,
    },
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    Vault(String),
}

impl RelocationError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::AmbiguousLink { .. } => "ambiguous-link",
            Self::ReadFile { .. } | Self::Vault(_) => "link-scan-error",
        }
    }
}

impl fmt::Display for RelocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AmbiguousLink { path, line, target } => write!(
                formatter,
                "ambiguous-link at {}:{line}: {target:?} can refer to both moved and staying content",
                path.display()
            ),
            Self::ReadFile { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::Vault(message) => formatter.write_str(message),
        }
    }
}

impl Error for RelocationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadFile { source, .. } => Some(source),
            Self::AmbiguousLink { .. } | Self::Vault(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;
    use crate::native::prose::section_model::{select_section, Selector};

    fn area_section<'a>(
        path: &Path,
        text: &'a str,
        title: &str,
    ) -> ProseSection<'a> {
        select_section(path, text, Selector::Section(title), |_| None)
            .expect("eligible area section")
    }

    #[test]
    fn extract_relocates_host_and_vault_links_without_touching_protected_text()
    {
        let root = tempfile::tempdir().expect("vault");
        fs::create_dir_all(root.path().join("done")).expect("done folder");
        fs::create_dir_all(root.path().join("2026")).expect("daily folder");
        let owner = "---\ntype: [[area]]\n---\n\
## Topic\n\
\n\
Inside [[#Moved Heading]] [[#Stay]] [[#Topic]] [[#^moved]] [[#^kept]]\n\
\n\
### Moved Heading\n\
\n\
Moved words ^moved\n\
\n\
## Stay\n\
\n\
Kept words ^kept\n\
\n\
## Elsewhere\n\
\n\
Outside [[#Moved Heading]]\n";
        fs::write(root.path().join("project.md"), owner).expect("owner");
        let external = "---\nlink: \"[[project#Moved Heading]]\"\n---\n\
[[project#Moved Heading|Alias]] ![[project#Moved Heading]]\n\
[Read](../project.md#Moved%20Heading)\n\
[[project#^moved]]\n\
```md\n[[project#Moved Heading]]\n```\n";
        fs::write(root.path().join("done/day.md"), external)
            .expect("external linker");
        fs::write(
            root.path().join("2026/20261127.md"),
            "[[project#Moved Heading]]\n",
        )
        .expect("daily linker");

        let section = area_section(Path::new("project.md"), owner, "Topic");
        let map = RelocationMap::new(&section, "project_topic");
        let plan = plan_vault(root.path(), &map, Direction::Extract, None)
            .expect("relocation plan");
        let owner_after = plan
            .files
            .iter()
            .find(|file| file.path == Path::new("project.md"))
            .expect("owner changed")
            .contents
            .as_str();
        assert!(owner_after.contains(
            "Inside [[#Moved Heading]] [[project#Stay]] [[project#Topic]] [[#^moved]] [[project#^kept]]"
        ), "{owner_after}");
        assert!(owner_after.contains("Outside [[project_topic#Moved Heading]]"));

        let external_after = plan
            .files
            .iter()
            .find(|file| file.path == Path::new("done/day.md"))
            .expect("done linker changed")
            .contents
            .as_str();
        assert!(
            external_after.contains("[[project_topic#Moved Heading|Alias]]")
        );
        assert!(external_after.contains("![[project_topic#Moved Heading]]"));
        assert!(external_after
            .contains("[Read](../project_topic.md#Moved%20Heading)"));
        assert!(external_after.contains("[[project_topic#^moved]]"));
        assert!(external_after.contains("link: \"[[project#Moved Heading]]\""));
        assert!(
            external_after.contains("```md\n[[project#Moved Heading]]\n```")
        );
        assert!(plan.links.iter().any(|link| {
            link.path == Path::new("done/day.md")
                && link.line == 5
                && link.before.contains("project.md#Moved%20Heading")
        }));
        assert!(plan.files.iter().any(|file| {
            file.path == Path::new("2026/20261127.md")
                && file.contents.contains("[[project_topic#Moved Heading]]")
        }));
    }

    #[test]
    fn heading_link_crossing_the_move_boundary_is_refused() {
        let root = tempfile::tempdir().expect("vault");
        let owner = "---\ntype: [[area]]\n---\n\
## Topic\n\
### Shared\n\
Moved.\n\
## Shared\n\
Stays.\n";
        fs::write(root.path().join("project.md"), owner).expect("owner");
        fs::write(root.path().join("other.md"), "[[project#Shared]]\n")
            .expect("linker");
        let section = area_section(Path::new("project.md"), owner, "Topic");
        let map = RelocationMap::new(&section, "project_topic");
        let error = plan_vault(root.path(), &map, Direction::Extract, None)
            .expect_err("mixed moved/staying target is ambiguous");
        assert_eq!(error.code(), "ambiguous-link");
        assert!(error.to_string().contains("other.md:1"));
    }

    #[test]
    fn inline_returns_child_links_to_the_host_and_skips_the_stub() {
        let root = tempfile::tempdir().expect("vault");
        fs::create_dir_all(root.path().join("done")).expect("done folder");
        let owner = "---\ntype: [[area]]\n---\n\
## Topic\n\
\n\
- [[project_topic|Topic]]\n\
\n\
## Idea\n\
\n\
Another idea.\n\
\n\
## Other\n\
\n\
Other prose.\n";
        let child = "---\ntype: [[prose]]\n---\n\
## Idea\n\
\n\
Child prose [[#Idea]] [[project#Other]]\n";
        fs::write(root.path().join("project.md"), owner).expect("owner");
        fs::write(root.path().join("project_topic.md"), child).expect("child");
        fs::write(
            root.path().join("done/day.md"),
            "[[project_topic#Idea|Read]]\n",
        )
        .expect("linker");
        let section = area_section(Path::new("project.md"), owner, "Topic");
        let stub_line = owner
            .lines()
            .position(|line| line == "- [[project_topic|Topic]]")
            .expect("stub line")
            + 1;
        let map = RelocationMap::for_inline(&section, "project_topic", child);
        let plan =
            plan_vault(root.path(), &map, Direction::Inline, Some(stub_line))
                .expect("inline link plan");

        let child_after = plan
            .files
            .iter()
            .find(|file| file.path == Path::new("project_topic.md"))
            .expect("child note changed")
            .contents
            .as_str();
        assert!(child_after.contains("Child prose [[#Topic#Idea]] [[#Other]]"));
        let external_after = plan
            .files
            .iter()
            .find(|file| file.path == Path::new("done/day.md"))
            .expect("external linker changed")
            .contents
            .as_str();
        assert!(external_after.contains("[[project#Topic#Idea|Read]]"));
        assert!(!plan.links.iter().any(|link| {
            link.path == Path::new("project.md") && link.line == stub_line
        }));
    }
}

fn resolve_fragment(
    fragment: &str,
    headings: &[PlannedHeading],
    allowed: Option<&BTreeSet<usize>>,
) -> Vec<usize> {
    let segments = fragment.split('#').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.iter().any(|segment| segment.trim().is_empty())
    {
        return Vec::new();
    }
    let all = (0..headings.len()).collect::<Vec<_>>();
    let mut candidates = allowed
        .map(|set| set.iter().copied().collect::<Vec<_>>())
        .unwrap_or(all);
    for (segment_index, raw_segment) in segments.iter().enumerate() {
        let segment = percent_decode(raw_segment);
        let exact = candidates
            .iter()
            .copied()
            .filter(|index| exact_heading(&headings[*index], &segment))
            .collect::<Vec<_>>();
        let matches = if exact.is_empty() {
            candidates
                .iter()
                .copied()
                .filter(|index| identity_heading(&headings[*index], &segment))
                .collect::<Vec<_>>()
        } else {
            exact
        };
        if matches.is_empty() {
            return Vec::new();
        }
        if segment_index + 1 == segments.len() {
            return matches;
        }
        let mut descendants = BTreeSet::new();
        for parent in matches {
            let parent_level = headings[parent].new_level;
            for index in parent + 1..headings.len() {
                if headings[index].new_level <= parent_level {
                    break;
                }
                if allowed.is_none_or(|set| set.contains(&index)) {
                    descendants.insert(index);
                }
            }
        }
        candidates = descendants.into_iter().collect();
    }
    Vec::new()
}

fn exact_heading(heading: &PlannedHeading, fragment: &str) -> bool {
    normalize_identity(&full_title(heading)) == normalize_identity(fragment)
}

fn identity_heading(heading: &PlannedHeading, fragment: &str) -> bool {
    let title = full_title(heading);
    heading_identity(&title) == heading_identity(fragment)
        || heading_plain_identity(&title) == heading_plain_identity(fragment)
}

fn full_title(heading: &PlannedHeading) -> String {
    heading.written.as_ref().map_or_else(
        || heading.title.clone(),
        |written| format!("{written} {}", heading.title),
    )
}

fn encode_fragment(
    fragment: &str,
    wikilink: bool,
    old_fragment: &str,
) -> String {
    let encode_spaces = !wikilink || old_fragment.contains("%20");
    fragment
        .split('#')
        .map(|segment| {
            let mut encoded = String::with_capacity(segment.len());
            for ch in segment.chars() {
                match ch {
                    '#' => encoded.push_str("%23"),
                    ' ' if encode_spaces => encoded.push_str("%20"),
                    '%' if old_fragment.contains('%') => {
                        encoded.push_str("%25")
                    }
                    _ => encoded.push(ch),
                }
            }
            encoded
        })
        .collect::<Vec<_>>()
        .join("#")
}

fn lookup_target(current_path: &Path, target: &str, wikilink: bool) -> PathBuf {
    let target_path = Path::new(target);
    if target.starts_with('/') {
        return target_path
            .strip_prefix("/")
            .unwrap_or(target_path)
            .to_path_buf();
    }
    let relative = target.starts_with("./")
        || target.starts_with("../")
        || (!wikilink
            && (target.contains('/') || !markdown_suffix(target).is_empty()));
    if !relative {
        return target_path.to_path_buf();
    }
    normalize_join(
        current_path.parent().unwrap_or_else(|| Path::new("")),
        target_path,
    )
}

fn normalize_join(base: &Path, target: &Path) -> PathBuf {
    let mut parts = Vec::new();
    for component in base.join(target).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !parts.is_empty() {
                    parts.pop();
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    parts.into_iter().collect()
}

fn relative_path(from: &Path, to: &Path) -> String {
    let from = from
        .components()
        .filter_map(normal_component)
        .collect::<Vec<_>>();
    let to = to
        .components()
        .filter_map(normal_component)
        .collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut result = vec!["..".to_string(); from.len().saturating_sub(common)];
    result.extend(to[common..].iter().cloned());
    if result.is_empty() {
        ".".to_string()
    } else {
        result.join("/")
    }
}

fn normal_component(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn markdown_suffix(note: &str) -> &'static str {
    if note.ends_with(".MD") {
        ".MD"
    } else if note.to_ascii_lowercase().ends_with(".md") {
        ".md"
    } else {
        ""
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn percent_encode_path(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~')
        {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_lowercase()
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

fn contains_casefolded_stem(text: &str, stem: &str) -> bool {
    if stem.is_empty() {
        return true;
    }
    percent_decode(text)
        .to_lowercase()
        .contains(&stem.to_lowercase())
}
