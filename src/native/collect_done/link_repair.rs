//! Obsidian link and dependency metadata repair.
use super::super::markdown;
use super::*;
use regex::Regex;
use std::sync::LazyLock;

pub(super) static DEPENDENCY_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)(?P<open>[\[(])(?P<prefix>[ \t]*)(?P<key>id|dependsOn)(?P<sep>[ \t]*::[ \t]*)(?P<value>[^\]\)\n]*)(?P<close>[\])])",
    )
    .expect("dependency metadata regex")
});
pub(crate) type MovedBlockTargets =
    BTreeMap<(PathBuf, String), MovedBlockTarget>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovedBlockTarget {
    pub(crate) archive_target: String,
    pub(crate) block_id: String,
    pub(crate) old_dependency_id: String,
    pub(crate) new_dependency_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRepairResult {
    pub(crate) contents: String,
    pub(crate) link_count: usize,
    pub(crate) dependency_metadata_count: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteIndex {
    pub(crate) relative_paths: BTreeSet<PathBuf>,
    pub(crate) basename_paths: BTreeMap<String, Option<PathBuf>>,
}

impl NoteIndex {
    pub(crate) fn from_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut relative_paths = BTreeSet::new();
        let mut basename_paths: BTreeMap<String, Option<PathBuf>> =
            BTreeMap::new();

        for path in paths {
            if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
                match basename_paths.entry(stem.to_lowercase()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Some(path.clone()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().as_ref() != Some(&path) {
                            entry.insert(None);
                        }
                    }
                }
            }
            relative_paths.insert(path);
        }

        Self {
            relative_paths,
            basename_paths,
        }
    }

    pub(crate) fn resolve(
        &self,
        current_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        if target.is_empty() {
            return Some(current_path.to_path_buf());
        }

        let candidate = target_to_relative_markdown_path(target)?;
        if self.relative_paths.contains(&candidate) {
            return Some(candidate);
        }

        let basename = target_basename(target);
        let stripped = strip_markdown_suffix_case_insensitive(basename);
        self.basename_paths
            .get(&stripped.to_lowercase())
            .and_then(|path| path.clone())
    }
}

/// Note-path resolver used by vault-wide moved-block link repair.
///
/// Archive discovery follows the shared case-insensitive [`NoteIndex`] rules
/// from `vault_links`: exact vault-relative paths match case-sensitively,
/// basename stems match case-insensitively. The mover supplies the shared
/// vault index without changing archive lookup rules.
pub(crate) trait ResolveMovedNote {
    fn resolve_moved_note(
        &self,
        current_path: &Path,
        target: &str,
    ) -> Option<PathBuf>;
}

impl ResolveMovedNote for NoteIndex {
    fn resolve_moved_note(
        &self,
        current_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        self.resolve(current_path, target)
    }
}

impl ResolveMovedNote for crate::native::vault_links::NoteIndex {
    fn resolve_moved_note(
        &self,
        current_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        crate::native::vault_links::NoteIndex::resolve(
            self,
            Some(current_path),
            target,
        )
    }
}
/// Fold link and dependency-metadata repairs into the plan.
///
/// Every byte comes from `captured`, the planning input set: no vault reads
/// happen here, so a note that changed underfoot is a missing entry (a
/// planning error) rather than a silently reread file.
pub(super) fn apply_link_repairs_to_plan(
    captured: &BTreeMap<PathBuf, String>,
    files: &mut [FilePlan],
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
    renames: &[RenamePlan],
) -> io::Result<Vec<LinkRepairPlan>> {
    let mut planned_contents = captured.clone();
    for file in files.iter() {
        if file.task_count > 0 || file.source_metadata_updated {
            planned_contents.insert(
                file.relative_source_path.clone(),
                file.source_contents.clone(),
            );
        }
        if let Some(archive_contents) = file.archive_contents.as_ref() {
            planned_contents.insert(
                file.relative_archive_path.clone(),
                archive_contents.clone(),
            );
        }
    }

    let note_paths: BTreeSet<PathBuf> =
        planned_contents.keys().cloned().collect();

    let moved_targets = moved_block_targets(
        &note_paths,
        &planned_contents,
        files,
        pairs,
        blocked_sources,
    )?;
    // Rename maintenance resolves against the pre-rename census so
    // shortened and case-variant targets still identify the old note; an
    // ambiguous basename never resolves and is left alone.
    let rename_map: BTreeMap<PathBuf, PathBuf> = renames
        .iter()
        .map(|rename| (rename.from.clone(), rename.to.clone()))
        .collect();
    let rename_dep_targets = rename_dependency_targets(captured, renames)?;
    let pre_rename_index = NoteIndex::from_paths(captured.keys().cloned());
    let removed_paths: BTreeSet<&PathBuf> =
        renames.iter().map(|rename| &rename.from).collect();
    if moved_targets.is_empty()
        && rename_map.is_empty()
        && rename_dep_targets.is_empty()
    {
        return Ok(Vec::new());
    }

    let note_index = NoteIndex::from_paths(note_paths.iter().cloned());
    let repair_paths = note_paths;

    let mut link_repair_counts = BTreeMap::new();
    for relative_path in repair_paths {
        if removed_paths.contains(&relative_path) {
            continue;
        }
        let contents = planned_contents
            .get(&relative_path)
            .cloned()
            .ok_or_else(|| missing_planned_contents(&relative_path))?;
        let mut repair = repair_links_in_note(
            &contents,
            &relative_path,
            &note_index,
            &moved_targets,
        );
        if !rename_map.is_empty() || !rename_dep_targets.is_empty() {
            let renamed = repair_rename_links(
                &repair.contents,
                &relative_path,
                &pre_rename_index,
                &rename_map,
            );
            repair.contents = renamed.contents;
            repair.link_count += renamed.link_count;
            let metadata = repair_dependency_metadata(
                &repair.contents,
                &rename_dep_targets,
            );
            repair.contents = metadata.contents;
            repair.dependency_metadata_count += metadata.count;
        }
        if let Some((source_target, stayed, stayed_headings)) =
            archive_stayed_block_ids(
                &relative_path,
                pairs,
                &planned_contents,
                &moved_targets,
                blocked_sources,
                files,
            )?
        {
            let pathless = repair_pathless_archive_links(
                &repair.contents,
                &source_target,
                &stayed,
                &stayed_headings,
            );
            repair.contents = pathless.contents;
            repair.link_count += pathless.link_count;
        }
        if repair.link_count > 0 || repair.dependency_metadata_count > 0 {
            // Moving a link to `done/` inside a still-drafted period
            // Review restamps its digest, so the archive never reads as
            // Bryan's edit.
            let repaired = crate::native::periods::note::carry_review_digest(
                &contents,
                &repair.contents,
            );
            planned_contents.insert(relative_path.clone(), repaired);
            link_repair_counts.insert(
                relative_path,
                (repair.link_count, repair.dependency_metadata_count),
            );
        }
    }

    for file in files.iter_mut() {
        let (source_link_repair_count, source_dependency_metadata_repair_count) =
            link_repair_counts
                .remove(&file.relative_source_path)
                .unwrap_or((0, 0));
        if source_link_repair_count > 0
            || source_dependency_metadata_repair_count > 0
        {
            file.source_contents = planned_contents
                .remove(&file.relative_source_path)
                .ok_or_else(|| {
                    missing_planned_contents(&file.relative_source_path)
                })?;
            file.source_link_repair_count = source_link_repair_count;
            file.source_dependency_metadata_repair_count =
                source_dependency_metadata_repair_count;
        } else if file.task_count > 0 || file.source_metadata_updated {
            file.source_contents = planned_contents
                .remove(&file.relative_source_path)
                .ok_or_else(|| {
                    missing_planned_contents(&file.relative_source_path)
                })?;
        }

        let (
            archive_link_repair_count,
            archive_dependency_metadata_repair_count,
        ) = link_repair_counts
            .remove(&file.relative_archive_path)
            .unwrap_or((0, 0));
        if archive_link_repair_count > 0
            || archive_dependency_metadata_repair_count > 0
        {
            file.archive_contents = Some(
                planned_contents
                    .remove(&file.relative_archive_path)
                    .ok_or_else(|| {
                        missing_planned_contents(&file.relative_archive_path)
                    })?,
            );
            file.archive_link_repair_count = archive_link_repair_count;
            file.archive_dependency_metadata_repair_count =
                archive_dependency_metadata_repair_count;
        } else if file.archive_contents.is_some() {
            file.archive_contents = Some(
                planned_contents
                    .remove(&file.relative_archive_path)
                    .ok_or_else(|| {
                        missing_planned_contents(&file.relative_archive_path)
                    })?,
            );
        }
    }

    let mut link_repairs = Vec::new();
    for (relative_path, contents) in planned_contents {
        if let Some((link_count, dependency_metadata_count)) =
            link_repair_counts.remove(&relative_path)
        {
            link_repairs.push(LinkRepairPlan {
                relative_path,
                contents,
                link_count,
                dependency_metadata_count,
                structural_regrouping: false,
            });
        }
    }
    link_repairs
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(link_repairs)
}
/// Old-to-new dependency ids for every anchor a renamed archive holds.
///
/// Block ids are preserved verbatim at the canonical destination; only the
/// path-qualified `[id::]` / `[dependsOn::]` identities change. Anchors
/// whose dependency id cannot be encoded are skipped, matching the existing
/// collision validation.
fn rename_dependency_targets(
    captured: &BTreeMap<PathBuf, String>,
    renames: &[RenamePlan],
) -> io::Result<MovedBlockTargets> {
    let mut targets = BTreeMap::new();
    for rename in renames {
        let old_contents =
            captured.get(&rename.from).cloned().unwrap_or_default();
        let new_target = vault_relative_link_target(&rename.to, "archive")?;
        // All content anchors count, not just trailing ids: archived rows
        // commonly carry `^id` mid-line before their close-date metadata.
        let old_ids: BTreeSet<String> =
            content_anchor_occurrences(&old_contents)
                .into_iter()
                .map(|occurrence| occurrence.id)
                .collect();
        for block_id in old_ids {
            let (Ok(old_dependency), Ok(new_dependency)) = (
                dependency_id(&rename.from, &block_id),
                dependency_id(&rename.to, &block_id),
            ) else {
                continue;
            };
            if old_dependency == new_dependency {
                continue;
            }
            targets.insert(
                (rename.from.clone(), block_id.clone()),
                MovedBlockTarget {
                    archive_target: new_target.clone(),
                    block_id: block_id.clone(),
                    old_dependency_id: old_dependency,
                    new_dependency_id: new_dependency,
                },
            );
        }
    }
    Ok(targets)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenameLinkRepair {
    contents: String,
    link_count: usize,
}

/// Retarget inbound note, heading, and block links to renamed archives.
///
/// Every supported link form is rewritten with alias, embed, and fragment
/// syntax preserved: `[[old]]`, `[[old#Heading]]`, `[[old#^id]]`,
/// `[text](old.md#^id)`, and their alias/embed variants. Resolution uses the
/// pre-rename index so shortened and case-variant targets still identify the
/// old note; an ambiguous basename never resolves and is left alone. Managed
/// YAML links are updated deliberately by the planner, so frontmatter stays
/// out of this generic pass, as do fenced blocks, inline code spans, and
/// `%%` / `<!-- -->` comments.
fn repair_rename_links(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    rename_map: &BTreeMap<PathBuf, PathBuf>,
) -> RenameLinkRepair {
    if rename_map.is_empty() {
        return RenameLinkRepair {
            contents: contents.to_string(),
            link_count: 0,
        };
    }
    let wiki = repair_rename_wiki_links(
        contents,
        current_path,
        note_index,
        rename_map,
    );
    let markdown = repair_rename_markdown_links(
        &wiki.contents,
        current_path,
        note_index,
        rename_map,
    );
    RenameLinkRepair {
        contents: markdown.contents,
        link_count: wiki.link_count + markdown.link_count,
    }
}

fn rename_skip_lines(contents: &str) -> BTreeSet<usize> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    super::super::markdown::non_content_line_indexes(&lines)
}

fn repair_rename_wiki_links(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    rename_map: &BTreeMap<PathBuf, PathBuf>,
) -> RenameLinkRepair {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let skipped = rename_skip_lines(contents);
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;

    while let Some(relative_start) = contents[cursor..].find("[[") {
        let start = cursor + relative_start;
        let inner_start = start + 2;
        let Some(relative_end) = contents[inner_start..].find("]]") else {
            break;
        };
        let end = inner_start + relative_end;

        repaired.push_str(&contents[cursor..start]);
        let inner = &contents[inner_start..end];
        let line_index = contents[..start].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..start].rfind('\n').map_or(0, |index| index + 1);
        let skip = skipped.contains(&line_index)
            || skip_repaired_span(
                true,
                line_index,
                line,
                start.saturating_sub(line_start),
                end + 2 - line_start,
                &fenced,
                frontmatter_end,
            );
        let replacement = if skip {
            None
        } else {
            rename_wiki_target(inner, current_path, note_index, rename_map)
        };
        if let Some(new_inner) = replacement {
            repaired.push_str("[[");
            repaired.push_str(&new_inner);
            repaired.push_str("]]");
            link_count += 1;
        } else {
            repaired.push_str(&contents[start..end + 2]);
        }
        cursor = end + 2;
    }

    repaired.push_str(&contents[cursor..]);
    RenameLinkRepair {
        contents: repaired,
        link_count,
    }
}

/// Resolve a wiki inner target against the pre-rename index and retarget it
/// when it names a renamed archive, preserving alias and fragment.
fn rename_wiki_target(
    inner: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    rename_map: &BTreeMap<PathBuf, PathBuf>,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (note_target, fragment) = match target_with_fragment.find('#') {
        Some(index) => (
            &target_with_fragment[..index],
            &target_with_fragment[index..],
        ),
        None => (target_with_fragment, ""),
    };
    if note_target.trim().is_empty() {
        return None;
    }
    let resolved = note_index.resolve(current_path, note_target.trim())?;
    let new_path = rename_map.get(&resolved)?;
    let new_target = vault_relative_link_target(new_path, "archive").ok()?;
    Some(format!("{new_target}{fragment}{alias}"))
}

fn repair_rename_markdown_links(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    rename_map: &BTreeMap<PathBuf, PathBuf>,
) -> RenameLinkRepair {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let skipped = rename_skip_lines(contents);
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;

    while let Some(relative_open) = contents[cursor..].find("](") {
        let open = cursor + relative_open;
        if let Some(wiki_start) = next_wiki_link_start(contents, cursor)
            && wiki_start < open
        {
            let Some(wiki_end) = contents[wiki_start + 2..].find("]]") else {
                break;
            };
            let wiki_end = wiki_start + 2 + wiki_end + 2;
            repaired.push_str(&contents[cursor..wiki_end]);
            cursor = wiki_end;
            continue;
        }

        let destination_start = open + 2;
        let Some(relative_close) = contents[destination_start..].find(')')
        else {
            break;
        };
        let destination_end = destination_start + relative_close;

        repaired.push_str(&contents[cursor..destination_start]);
        let destination = &contents[destination_start..destination_end];
        let line_index = contents[..open].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..open].rfind('\n').map_or(0, |index| index + 1);
        let skip = skipped.contains(&line_index)
            || skip_repaired_span(
                true,
                line_index,
                line,
                open.saturating_sub(line_start),
                destination_end + 1 - line_start,
                &fenced,
                frontmatter_end,
            );
        if skip {
            repaired.push_str(destination);
        } else if let Some(new_destination) = rename_markdown_destination(
            destination,
            current_path,
            note_index,
            rename_map,
        ) {
            repaired.push_str(&new_destination);
            link_count += 1;
        } else {
            repaired.push_str(destination);
        }
        repaired.push(')');
        cursor = destination_end + 1;
    }

    repaired.push_str(&contents[cursor..]);
    RenameLinkRepair {
        contents: repaired,
        link_count,
    }
}

/// Retarget a Markdown destination naming a renamed archive, preserving the
/// fragment and the bare/`.md` style of the original target.
fn rename_markdown_destination(
    destination: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    rename_map: &BTreeMap<PathBuf, PathBuf>,
) -> Option<String> {
    if !is_simple_markdown_destination(destination) {
        return None;
    }
    let (target, fragment) = match destination.find('#') {
        Some(index) => (&destination[..index], &destination[index..]),
        None => (destination, ""),
    };
    if target.contains("://") || target.trim().is_empty() {
        return None;
    }
    let resolved = note_index.resolve(current_path, target.trim())?;
    let new_path = rename_map.get(&resolved)?;
    let new_target = vault_relative_link_target(new_path, "archive").ok()?;
    if has_markdown_extension(target.trim()) {
        Some(format!("{new_target}.md{fragment}"))
    } else {
        Some(format!("{new_target}{fragment}"))
    }
}

pub(super) fn moved_block_targets(
    note_paths: &BTreeSet<PathBuf>,
    planned_contents: &BTreeMap<PathBuf, String>,
    files: &[FilePlan],
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
) -> io::Result<MovedBlockTargets> {
    let mut targets = archived_block_targets(
        note_paths,
        planned_contents,
        pairs,
        blocked_sources,
    )?;
    overlay_current_run_block_targets(files, &mut targets)?;
    Ok(targets)
}
pub(super) fn archived_block_targets(
    note_paths: &BTreeSet<PathBuf>,
    planned_contents: &BTreeMap<PathBuf, String>,
    pairs: &ArchivePairIndex,
    blocked_sources: &BTreeSet<PathBuf>,
) -> io::Result<MovedBlockTargets> {
    let mut targets = BTreeMap::new();
    for archive_relative_path in note_paths {
        let ArchiveLookup::Bound(bound) =
            pairs.for_archive(archive_relative_path)
        else {
            continue;
        };
        if blocked_sources.contains(&bound.source) {
            continue;
        }
        let source_relative_path = bound.source;
        if !note_paths.contains(&source_relative_path) {
            continue;
        }

        let archive_contents =
            note_contents(archive_relative_path, planned_contents)?;

        let source_contents =
            note_contents(&source_relative_path, planned_contents)?;
        let source_block_ids: BTreeSet<String> =
            block_ids_in_markdown(&source_contents)
                .into_iter()
                .collect();
        let mut archive_block_id_counts = BTreeMap::new();
        for block_id in block_ids_in_markdown(&archive_contents) {
            *archive_block_id_counts.entry(block_id).or_insert(0usize) += 1;
        }

        let archive_target =
            vault_relative_link_target(archive_relative_path, "archive")?;
        for (block_id, count) in archive_block_id_counts {
            if count > 1 || source_block_ids.contains(&block_id) {
                continue;
            }
            let (Ok(old_dependency_id), Ok(new_dependency_id)) = (
                dependency_id(&source_relative_path, &block_id),
                dependency_id(archive_relative_path, &block_id),
            ) else {
                continue;
            };
            targets.insert(
                (source_relative_path.clone(), block_id.clone()),
                MovedBlockTarget {
                    archive_target: archive_target.clone(),
                    old_dependency_id,
                    new_dependency_id,
                    block_id,
                },
            );
        }
    }
    Ok(targets)
}
pub(super) fn overlay_current_run_block_targets(
    files: &[FilePlan],
    targets: &mut MovedBlockTargets,
) -> io::Result<()> {
    for file in files.iter().filter(|file| file.task_count > 0) {
        let archive_target =
            vault_relative_link_target(&file.relative_archive_path, "archive")?;
        for final_id in &file.moved_block_final_ids {
            if !file.moved_block_ids.contains(final_id) {
                targets.remove(&(
                    file.relative_source_path.clone(),
                    final_id.clone(),
                ));
            }
        }
        for block_id in &file.moved_block_ids {
            if file.ambiguous_moved_block_ids.contains(block_id) {
                targets.remove(&(
                    file.relative_source_path.clone(),
                    block_id.clone(),
                ));
                continue;
            }
            let final_block_id = file
                .moved_block_id_final_ids
                .get(block_id)
                .cloned()
                .unwrap_or_else(|| block_id.clone());
            targets.insert(
                (file.relative_source_path.clone(), block_id.clone()),
                MovedBlockTarget {
                    archive_target: archive_target.clone(),
                    old_dependency_id: dependency_id(
                        &file.relative_source_path,
                        block_id,
                    )?,
                    new_dependency_id: dependency_id(
                        &file.relative_archive_path,
                        &final_block_id,
                    )?,
                    block_id: final_block_id,
                },
            );
        }
    }
    Ok(())
}
/// Read a planning input from the captured set.
///
/// There is no vault fallback: every path the planner touches was captured
/// up front, so a missing entry means the vault changed underfoot.
pub(super) fn note_contents(
    relative_path: &Path,
    planned_contents: &BTreeMap<PathBuf, String>,
) -> io::Result<String> {
    planned_contents
        .get(relative_path)
        .cloned()
        .ok_or_else(|| missing_planned_contents(relative_path))
}
pub(crate) fn source_relative_path_from_archive(
    archive_relative_path: &Path,
) -> Option<PathBuf> {
    let archive_note_path =
        archive_relative_path.strip_prefix(Path::new("done")).ok()?;
    let source_stem = archive_note_path
        .file_stem()
        .and_then(OsStr::to_str)?
        .strip_suffix("_done")?;
    let mut source_relative_path =
        archive_note_path.with_file_name(format!("{source_stem}.md"));
    source_relative_path = source_relative_path.components().collect();
    is_normal_relative_path(&source_relative_path)
        .then_some(source_relative_path)
}
pub(crate) fn archive_parent_target(contents: &str) -> Option<String> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    if lines
        .first()
        .map(|line| is_frontmatter_marker(split_line_ending(line).0))
        != Some(true)
    {
        return None;
    }

    let closing_index =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            is_frontmatter_marker(split_line_ending(line).0).then_some(index)
        })?;
    for line in lines.iter().take(closing_index).skip(1) {
        let (content, _) = split_line_ending(line);
        let Some(value) = content.trim_start().strip_prefix("parent:") else {
            continue;
        };
        return frontmatter_wiki_target(value.trim()).map(str::to_string);
    }
    None
}
pub(super) fn frontmatter_wiki_target(value: &str) -> Option<&str> {
    let unquoted = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value);
    let inner = unquoted.strip_prefix("[[")?.strip_suffix("]]")?;
    let without_alias =
        inner.split_once('|').map_or(inner, |(target, _)| target);
    Some(
        without_alias
            .split_once('#')
            .map_or(without_alias, |(target, _)| target),
    )
}
pub(crate) fn repair_links_in_note(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> LinkRepairResult {
    repair_links_in_note_with(
        contents,
        current_path,
        note_index,
        moved_targets,
        true,
    )
}

/// Vault-wide moved-block repair with an explicit resolver and code-context flag.
///
/// `skip_code` leaves fenced blocks, frontmatter, and inline code spans
/// untouched. Archive callers pass `skip_code = true`.
pub(crate) fn repair_links_in_note_with<R: ResolveMovedNote>(
    contents: &str,
    current_path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
    skip_code: bool,
) -> LinkRepairResult {
    let wiki_repair = repair_wiki_links(
        contents,
        current_path,
        note_index,
        moved_targets,
        skip_code,
    );
    let markdown_repair = repair_markdown_links(
        &wiki_repair.contents,
        current_path,
        note_index,
        moved_targets,
        skip_code,
    );
    let metadata_repair =
        repair_dependency_metadata(&markdown_repair.contents, moved_targets);

    LinkRepairResult {
        contents: metadata_repair.contents,
        link_count: wiki_repair.link_count + markdown_repair.link_count,
        dependency_metadata_count: metadata_repair.count,
    }
}

/// Source note and stayed-behind block ids plus headings for an archive note.
///
/// When a task block is archived, its Depends-On lines and legacy
/// children move with it, but a pathless `[[#^x]]` whose target stayed
/// behind would silently re-resolve inside the archive note. Returns the
/// source link target plus the block ids and headings that stayed when
/// `current_path` is an archive note whose parent source exists.
pub(super) fn archive_stayed_block_ids(
    current_path: &Path,
    pairs: &ArchivePairIndex,
    planned_contents: &BTreeMap<PathBuf, String>,
    moved_targets: &MovedBlockTargets,
    blocked_sources: &BTreeSet<PathBuf>,
    files: &[FilePlan],
) -> io::Result<Option<(String, BTreeSet<String>, BTreeSet<String>)>> {
    let source_relative_path = match pairs.for_archive(current_path) {
        ArchiveLookup::Bound(bound) => {
            if blocked_sources.contains(&bound.source) {
                return Ok(None);
            }
            bound.source
        }
        ArchiveLookup::Refused { .. } => return Ok(None),
        ArchiveLookup::Orphan { .. } => {
            // Newly created archives are absent from the captured pair
            // census; the current FilePlan is the association.
            match files
                .iter()
                .find(|file| file.relative_archive_path == current_path)
            {
                Some(file)
                    if !blocked_sources
                        .contains(&file.relative_source_path) =>
                {
                    file.relative_source_path.clone()
                }
                _ => return Ok(None),
            }
        }
    };
    let source_contents =
        note_contents(&source_relative_path, planned_contents)?;
    let moved: BTreeSet<String> = moved_targets
        .keys()
        .filter(|(path, _)| path == &source_relative_path)
        .map(|(_, block_id)| block_id.clone())
        .collect();
    let stayed: BTreeSet<String> = block_ids_in_markdown(&source_contents)
        .into_iter()
        .filter(|block_id| !moved.contains(block_id))
        .collect();
    let stayed_headings = headings_in_markdown(&source_contents);
    if stayed.is_empty() && stayed_headings.is_empty() {
        return Ok(None);
    }
    let source_target =
        vault_relative_link_target(&source_relative_path, "source")?;
    Ok(Some((source_target, stayed, stayed_headings)))
}

/// ATX headings that stayed in a note, for pathless heading-link repair.
///
/// Collects `markdown::atx_heading` titles outside frontmatter, fences, and
/// multiline comments. Titles are stored lowercased for case-insensitive
/// Obsidian heading matching; the original link fragment is preserved on
/// rewrite.
pub(super) fn headings_in_markdown(contents: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let masked = markdown::non_content_line_indexes(&lines);
    let mut headings = BTreeSet::new();
    for (index, line) in logical.iter().enumerate() {
        if masked.contains(&index) {
            continue;
        }
        if let Some((_, title)) = markdown::atx_heading(line) {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                headings.insert(trimmed.to_lowercase());
            }
        }
    }
    headings
}

/// Qualify pathless block and heading links that stayed behind.
///
/// Rewrites `[[#^x]]` (with optional alias and `!`) to
/// `[[<source>#^x]]` when `x` stayed in the source note, and rewrites a
/// pathless heading link `[[#Heading]]` to `[[<source>#Heading]]` when that
/// heading stayed in the source, so archived Depends-On lines and child
/// links keep pointing at the target that did not move. Fenced code and
/// inline code spans are untouched.
pub(super) fn repair_pathless_archive_links(
    contents: &str,
    source_target: &str,
    stayed: &BTreeSet<String>,
    stayed_headings: &BTreeSet<String>,
) -> LinkRepairResult {
    let lines = contents.lines().collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    for (line_index, segment) in contents.split_inclusive('\n').enumerate() {
        let (line, ending) = split_line_ending(segment);
        if fenced.contains(&line_index) {
            repaired.push_str(segment);
            continue;
        }
        let code_spans =
            super::super::task_dependencies::inline_code_spans(line);
        let mut cursor = 0;
        let mut rewritten = String::with_capacity(line.len());
        while let Some(relative_start) = line[cursor..].find("[[") {
            let start = cursor + relative_start;
            let inner_start = start + 2;
            let Some(relative_end) = line[inner_start..].find("]]") else {
                break;
            };
            let end = inner_start + relative_end;
            let candidate = &line[inner_start..end];
            let replacement = if code_spans
                .iter()
                .any(|span| start < span.end && end + 2 > span.start)
            {
                None
            } else {
                qualify_stayed_link(
                    candidate,
                    source_target,
                    stayed,
                    stayed_headings,
                )
            };
            if let Some(new_inner) = replacement {
                rewritten.push_str(&line[cursor..start]);
                rewritten.push_str("[[");
                rewritten.push_str(&new_inner);
                rewritten.push_str("]]");
                link_count += 1;
            } else {
                rewritten.push_str(&line[cursor..end + 2]);
            }
            cursor = end + 2;
        }
        rewritten.push_str(&line[cursor..]);
        repaired.push_str(&rewritten);
        repaired.push_str(ending);
    }
    LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn qualify_stayed_link(
    inner: &str,
    source_target: &str,
    stayed: &BTreeSet<String>,
    stayed_headings: &BTreeSet<String>,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    if let Some((note_target, fragment)) =
        split_block_fragment(target_with_fragment)
    {
        if !note_target.trim().is_empty() {
            return None;
        }
        let block_id = fragment.strip_prefix('^')?;
        if !stayed.contains(block_id) {
            return None;
        }
        return Some(format!("{source_target}#^{block_id}{alias}"));
    }
    let (note_target, fragment) = target_with_fragment.split_once('#')?;
    if !note_target.trim().is_empty() {
        return None;
    }
    if fragment.is_empty() || fragment.starts_with('^') {
        return None;
    }
    if !stayed_headings.contains(&fragment.trim().to_lowercase()) {
        return None;
    }
    Some(format!("{source_target}#{fragment}{alias}"))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyMetadataRepair {
    pub(crate) contents: String,
    pub(crate) count: usize,
}
pub(crate) fn repair_dependency_metadata(
    contents: &str,
    moved_targets: &MovedBlockTargets,
) -> DependencyMetadataRepair {
    let replacements: BTreeMap<&str, &str> = moved_targets
        .values()
        .filter(|target| target.old_dependency_id != target.new_dependency_id)
        .map(|target| {
            (
                target.old_dependency_id.as_str(),
                target.new_dependency_id.as_str(),
            )
        })
        .collect();
    if replacements.is_empty() {
        return DependencyMetadataRepair {
            contents: contents.to_string(),
            count: 0,
        };
    }

    let mut count = 0usize;
    let lines = contents.lines().collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut next = String::with_capacity(contents.len());
    for (line_index, line_with_ending) in
        contents.split_inclusive('\n').enumerate()
    {
        let (line, ending) = split_line_ending(line_with_ending);
        if fenced.contains(&line_index) {
            next.push_str(line_with_ending);
            continue;
        }
        let code_spans =
            super::super::task_dependencies::inline_code_spans(line);
        let mut cursor = 0;
        for captures in DEPENDENCY_FIELD_RE.captures_iter(line) {
            let whole = captures.get(0).expect("whole dependency field");
            if code_spans.iter().any(|span| span.contains(&whole.start()))
                || !matches!(
                    (
                        captures.name("open").unwrap().as_str(),
                        captures.name("close").unwrap().as_str()
                    ),
                    ("[", "]") | ("(", ")")
                )
            {
                continue;
            }
            next.push_str(&line[cursor..whole.start()]);
            let key = captures.name("key").map_or("", |value| value.as_str());
            let value =
                captures.name("value").map_or("", |value| value.as_str());
            let mut replacement_value = None;
            if key == "id" {
                let trimmed = value.trim();
                if let Some(replacement) = replacements.get(trimmed) {
                    count += 1;
                    let leading_len = value.len() - value.trim_start().len();
                    let trailing_start = value.trim_end().len();
                    replacement_value = Some(format!(
                        "{}{}{}",
                        &value[..leading_len],
                        replacement,
                        &value[trailing_start..]
                    ));
                }
            } else {
                let mut changed = false;
                let value = value
                    .split(',')
                    .map(|segment| {
                        let trimmed = segment.trim();
                        let Some(replacement) = replacements.get(trimmed)
                        else {
                            return segment.to_string();
                        };
                        changed = true;
                        count += 1;
                        let leading_len =
                            segment.len() - segment.trim_start().len();
                        let trailing_start = segment.trim_end().len();
                        format!(
                            "{}{}{}",
                            &segment[..leading_len],
                            replacement,
                            &segment[trailing_start..]
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                if changed {
                    replacement_value = Some(value);
                }
            }
            if let Some(value) = replacement_value {
                next.push_str(captures.name("open").unwrap().as_str());
                next.push_str(captures.name("prefix").unwrap().as_str());
                next.push_str(key);
                next.push_str(captures.name("sep").unwrap().as_str());
                next.push_str(&value);
                next.push_str(captures.name("close").unwrap().as_str());
            } else {
                next.push_str(whole.as_str());
            }
            cursor = whole.end();
        }
        next.push_str(&line[cursor..]);
        next.push_str(ending);
    }
    DependencyMetadataRepair {
        contents: next,
        count,
    }
}
pub(super) fn repair_wiki_links<R: ResolveMovedNote>(
    contents: &str,
    current_path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
    skip_code: bool,
) -> LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;

    while let Some(relative_start) = contents[cursor..].find("[[") {
        let start = cursor + relative_start;
        let inner_start = start + 2;
        let Some(relative_end) = contents[inner_start..].find("]]") else {
            break;
        };
        let end = inner_start + relative_end;

        repaired.push_str(&contents[cursor..start]);
        let inner = &contents[inner_start..end];
        let line_index = contents[..start].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..start].rfind('\n').map_or(0, |index| index + 1);
        let skip = skip_repaired_span(
            skip_code,
            line_index,
            line,
            start.saturating_sub(line_start),
            end + 2 - line_start,
            &fenced,
            frontmatter_end,
        );
        let replacement = if skip {
            None
        } else {
            repair_wiki_link_inner(
                inner,
                current_path,
                note_index,
                moved_targets,
            )
        };
        if let Some(new_inner) = replacement {
            repaired.push_str("[[");
            repaired.push_str(&new_inner);
            repaired.push_str("]]");
            link_count += 1;
        } else {
            repaired.push_str(&contents[start..end + 2]);
        }
        cursor = end + 2;
    }

    repaired.push_str(&contents[cursor..]);
    LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}
pub(crate) fn repair_wiki_link_inner<R: ResolveMovedNote>(
    inner: &str,
    current_path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (target, fragment) = split_block_fragment(target_with_fragment)?;
    let resolved_path = note_index.resolve_moved_note(current_path, target)?;
    let block_id = fragment.strip_prefix('^')?;
    let moved_target =
        moved_targets.get(&(resolved_path, block_id.to_string()))?;
    Some(format!(
        "{}#^{}{}",
        moved_target.archive_target, moved_target.block_id, alias
    ))
}
pub(super) fn repair_markdown_links<R: ResolveMovedNote>(
    contents: &str,
    current_path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
    skip_code: bool,
) -> LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;

    while let Some(relative_open) = contents[cursor..].find("](") {
        let open = cursor + relative_open;
        if let Some(wiki_start) = next_wiki_link_start(contents, cursor)
            && wiki_start < open
        {
            let Some(wiki_end) = contents[wiki_start + 2..].find("]]") else {
                break;
            };
            let wiki_end = wiki_start + 2 + wiki_end + 2;
            repaired.push_str(&contents[cursor..wiki_end]);
            cursor = wiki_end;
            continue;
        }

        let destination_start = open + 2;
        let Some(relative_close) = contents[destination_start..].find(')')
        else {
            break;
        };
        let destination_end = destination_start + relative_close;

        repaired.push_str(&contents[cursor..destination_start]);
        let destination = &contents[destination_start..destination_end];
        let line_index = contents[..open].matches('\n').count();
        let line = lines.get(line_index).copied().unwrap_or("");
        let line_start =
            contents[..open].rfind('\n').map_or(0, |index| index + 1);
        let skip = skip_repaired_span(
            skip_code,
            line_index,
            line,
            open.saturating_sub(line_start),
            destination_end + 1 - line_start,
            &fenced,
            frontmatter_end,
        );
        if skip {
            repaired.push_str(destination);
        } else if let Some(new_destination) = repair_markdown_destination(
            destination,
            current_path,
            note_index,
            moved_targets,
        ) {
            repaired.push_str(&new_destination);
            link_count += 1;
        } else {
            repaired.push_str(destination);
        }
        repaired.push(')');
        cursor = destination_end + 1;
    }

    repaired.push_str(&contents[cursor..]);
    LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn skip_repaired_span(
    skip_code: bool,
    line_index: usize,
    line: &str,
    start_in_line: usize,
    end_in_line: usize,
    fenced: &BTreeSet<usize>,
    frontmatter_end: Option<usize>,
) -> bool {
    if !skip_code {
        return false;
    }
    if fenced.contains(&line_index)
        || frontmatter_end.is_some_and(|end| line_index <= end)
    {
        return true;
    }
    super::super::task_dependencies::inline_code_spans(line)
        .iter()
        .any(|span| start_in_line < span.end && end_in_line > span.start)
}
pub(super) fn next_wiki_link_start(
    contents: &str,
    cursor: usize,
) -> Option<usize> {
    contents[cursor..]
        .find("[[")
        .map(|relative_start| cursor + relative_start)
}
pub(crate) fn repair_markdown_destination<R: ResolveMovedNote>(
    destination: &str,
    current_path: &Path,
    note_index: &R,
    moved_targets: &MovedBlockTargets,
) -> Option<String> {
    if !is_simple_markdown_destination(destination) {
        return None;
    }

    let (target, fragment) = split_block_fragment(destination)?;
    if target.contains("://") {
        return None;
    }

    let resolved_path = note_index.resolve_moved_note(current_path, target)?;
    let block_id = fragment.strip_prefix('^')?;
    let moved_target =
        moved_targets.get(&(resolved_path, block_id.to_string()))?;
    let new_destination_target =
        if target.is_empty() || has_markdown_extension(target) {
            format!("{}.md", moved_target.archive_target)
        } else {
            moved_target.archive_target.clone()
        };
    Some(format!(
        "{new_destination_target}#^{}",
        moved_target.block_id
    ))
}
pub(crate) fn split_block_fragment(target: &str) -> Option<(&str, &str)> {
    let (note_target, fragment) = target.split_once('#')?;
    is_block_fragment(fragment).then_some((note_target, fragment))
}
pub(super) fn is_block_fragment(fragment: &str) -> bool {
    let Some(block_id) = fragment.strip_prefix('^') else {
        return false;
    };
    !block_id.is_empty() && block_id.bytes().all(is_block_id_byte)
}
pub(crate) fn is_simple_markdown_destination(destination: &str) -> bool {
    !destination.is_empty()
        && !destination.contains(char::is_whitespace)
        && !destination.contains('(')
        && !destination.contains(')')
}
pub(super) fn target_to_relative_markdown_path(
    target: &str,
) -> Option<PathBuf> {
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains(':')
    {
        return None;
    }

    let target_with_extension = if has_markdown_extension(target) {
        target.to_string()
    } else {
        format!("{target}.md")
    };
    let path = PathBuf::from(target_with_extension);
    is_normal_relative_path(&path).then_some(path)
}
pub(super) fn has_markdown_extension(target: &str) -> bool {
    target
        .rsplit_once('/')
        .map(|(_, file_name)| file_name)
        .unwrap_or(target)
        .to_ascii_lowercase()
        .ends_with(".md")
}
fn target_basename(target: &str) -> &str {
    target.rsplit(['/', '\\']).next().unwrap_or(target)
}
fn strip_markdown_suffix_case_insensitive(name: &str) -> &str {
    if name.len() >= 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md") {
        &name[..name.len() - 3]
    } else {
        name
    }
}
pub(super) fn is_normal_relative_path(path: &Path) -> bool {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            _ => return false,
        }
    }
    has_component
}
pub(super) fn vault_relative_path(
    vault: &Path,
    path: &Path,
    path_kind: &str,
) -> io::Result<PathBuf> {
    path.strip_prefix(vault)
        .map(Path::to_path_buf)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{path_kind} path {} is outside vault {}: {error}",
                    path.display(),
                    vault.display()
                ),
            )
        })
}
pub(super) fn missing_planned_contents(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("missing planned contents for {}", path.display()),
    )
}
