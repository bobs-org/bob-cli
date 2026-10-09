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
pub(super) type MovedBlockTargets =
    BTreeMap<(PathBuf, String), MovedBlockTarget>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MovedBlockTarget {
    pub(super) archive_target: String,
    pub(super) block_id: String,
    pub(super) old_dependency_id: String,
    pub(super) new_dependency_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LinkRepairResult {
    pub(super) contents: String,
    pub(super) link_count: usize,
    pub(super) dependency_metadata_count: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoteIndex {
    pub(super) relative_paths: BTreeSet<PathBuf>,
    pub(super) basename_paths: BTreeMap<String, Option<PathBuf>>,
}

impl NoteIndex {
    pub(super) fn from_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut relative_paths = BTreeSet::new();
        let mut basename_paths = BTreeMap::new();

        for path in paths {
            if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
                match basename_paths.entry(stem.to_string()) {
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

    pub(super) fn resolve(
        &self,
        current_path: &Path,
        target: &str,
    ) -> Option<PathBuf> {
        if target.is_empty() {
            return Some(current_path.to_path_buf());
        }

        let candidate = target_to_relative_markdown_path(target)?;
        let target_has_path = target.contains('/');
        let target_has_markdown_extension = has_markdown_extension(target);
        if (target_has_path || target_has_markdown_extension)
            && self.relative_paths.contains(&candidate)
        {
            return Some(candidate);
        }

        if target_has_path {
            return None;
        }

        let basename = target
            .strip_suffix(".md")
            .or_else(|| target.strip_suffix(".MD"))
            .unwrap_or(target);
        self.basename_paths
            .get(basename)
            .and_then(|path| path.clone())
    }
}
pub(super) fn apply_link_repairs_to_plan(
    vault: &Path,
    files: &mut [FilePlan],
) -> io::Result<Vec<LinkRepairPlan>> {
    let note_files = link_repair_markdown_files(vault)?;
    let mut disk_note_paths = BTreeSet::new();
    for path in &note_files {
        disk_note_paths.insert(vault_relative_path(vault, path, "note")?);
    }

    let mut planned_contents = BTreeMap::new();
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

    let mut note_paths = disk_note_paths;
    note_paths.extend(planned_contents.keys().cloned());

    let moved_targets =
        moved_block_targets(vault, &note_paths, &planned_contents, files)?;
    if moved_targets.is_empty() {
        return Ok(Vec::new());
    }

    let note_index = NoteIndex::from_paths(note_paths.iter().cloned());
    let repair_paths = note_paths;

    let mut link_repair_counts = BTreeMap::new();
    for relative_path in repair_paths {
        let contents = match planned_contents.get(&relative_path) {
            Some(contents) => contents.clone(),
            None => fs::read_to_string(vault.join(&relative_path))?,
        };
        let mut repair = repair_links_in_note(
            &contents,
            &relative_path,
            &note_index,
            &moved_targets,
        );
        if let Some((source_target, stayed)) = archive_stayed_block_ids(
            vault,
            &relative_path,
            &contents,
            &note_index,
            &planned_contents,
            &moved_targets,
        )? {
            let pathless = repair_pathless_archive_links(
                &repair.contents,
                &source_target,
                &stayed,
            );
            repair.contents = pathless.contents;
            repair.link_count += pathless.link_count;
        }
        if repair.link_count > 0 || repair.dependency_metadata_count > 0 {
            planned_contents.insert(relative_path.clone(), repair.contents);
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
            });
        }
    }
    link_repairs
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(link_repairs)
}
pub(super) fn moved_block_targets(
    vault: &Path,
    note_paths: &BTreeSet<PathBuf>,
    planned_contents: &BTreeMap<PathBuf, String>,
    files: &[FilePlan],
) -> io::Result<MovedBlockTargets> {
    let mut targets =
        archived_block_targets(vault, note_paths, planned_contents)?;
    overlay_current_run_block_targets(files, &mut targets)?;
    Ok(targets)
}
pub(super) fn archived_block_targets(
    vault: &Path,
    note_paths: &BTreeSet<PathBuf>,
    planned_contents: &BTreeMap<PathBuf, String>,
) -> io::Result<MovedBlockTargets> {
    let mut targets = BTreeMap::new();
    let note_index = NoteIndex::from_paths(note_paths.iter().cloned());
    for archive_relative_path in note_paths {
        let Some(source_relative_path) =
            source_relative_path_from_archive(archive_relative_path)
        else {
            continue;
        };
        if !note_paths.contains(&source_relative_path) {
            continue;
        }

        let archive_contents =
            note_contents(vault, archive_relative_path, planned_contents)?;
        if !archive_parent_matches_source(
            &archive_contents,
            archive_relative_path,
            &source_relative_path,
            &note_index,
        ) {
            continue;
        }

        let source_contents =
            note_contents(vault, &source_relative_path, planned_contents)?;
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
pub(super) fn note_contents(
    vault: &Path,
    relative_path: &Path,
    planned_contents: &BTreeMap<PathBuf, String>,
) -> io::Result<String> {
    if let Some(contents) = planned_contents.get(relative_path) {
        return Ok(contents.clone());
    }

    let path = vault.join(relative_path);
    fs::read_to_string(&path).map_err(|error| fs_error("read", &path, error))
}
pub(super) fn source_relative_path_from_archive(
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
pub(super) fn archive_parent_matches_source(
    archive_contents: &str,
    archive_relative_path: &Path,
    source_relative_path: &Path,
    note_index: &NoteIndex,
) -> bool {
    let Some(parent_target) = archive_parent_target(archive_contents) else {
        return false;
    };
    note_index
        .resolve(archive_relative_path, &parent_target)
        .as_deref()
        == Some(source_relative_path)
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
pub(super) fn repair_links_in_note(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> LinkRepairResult {
    let wiki_repair =
        repair_wiki_links(contents, current_path, note_index, moved_targets);
    let markdown_repair = repair_markdown_links(
        &wiki_repair.contents,
        current_path,
        note_index,
        moved_targets,
    );
    let metadata_repair =
        repair_dependency_metadata(&markdown_repair.contents, moved_targets);

    LinkRepairResult {
        contents: metadata_repair.contents,
        link_count: wiki_repair.link_count + markdown_repair.link_count,
        dependency_metadata_count: metadata_repair.count,
    }
}

/// Source note and stayed-behind block ids for an archive note.
///
/// When a task block is archived, its Depends-On lines and legacy
/// children move with it, but a pathless `[[#^x]]` whose target stayed
/// behind would silently re-resolve inside the archive note. Returns the
/// source link target plus the block ids that stayed when `current_path`
/// is an archive note whose parent source exists.
pub(super) fn archive_stayed_block_ids(
    vault: &Path,
    current_path: &Path,
    current_contents: &str,
    note_index: &NoteIndex,
    planned_contents: &BTreeMap<PathBuf, String>,
    moved_targets: &MovedBlockTargets,
) -> io::Result<Option<(String, BTreeSet<String>)>> {
    let Some(source_relative_path) =
        source_relative_path_from_archive(current_path)
    else {
        return Ok(None);
    };
    if !archive_parent_matches_source(
        current_contents,
        current_path,
        &source_relative_path,
        note_index,
    ) {
        return Ok(None);
    }
    let source_contents =
        note_contents(vault, &source_relative_path, planned_contents)?;
    let moved: BTreeSet<String> = moved_targets
        .keys()
        .filter(|(path, _)| path == &source_relative_path)
        .map(|(_, block_id)| block_id.clone())
        .collect();
    let stayed: BTreeSet<String> = block_ids_in_markdown(&source_contents)
        .into_iter()
        .filter(|block_id| !moved.contains(block_id))
        .collect();
    if stayed.is_empty() {
        return Ok(None);
    }
    let source_target =
        vault_relative_link_target(&source_relative_path, "source")?;
    Ok(Some((source_target, stayed)))
}

/// Qualify pathless block links that stayed behind.
///
/// Rewrites `[[#^x]]` (with optional alias and `!`) to
/// `[[<source>#^x]]` when `x` stayed in the source note, so archived
/// Depends-On lines and legacy children keep pointing at the target
/// that did not move. Fenced code and inline code spans are untouched.
pub(super) fn repair_pathless_archive_links(
    contents: &str,
    source_target: &str,
    stayed: &BTreeSet<String>,
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
                qualify_stayed_link(candidate, source_target, stayed)
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
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (note_target, fragment) = split_block_fragment(target_with_fragment)?;
    if !note_target.trim().is_empty() {
        return None;
    }
    let block_id = fragment.strip_prefix('^')?;
    if !stayed.contains(block_id) {
        return None;
    }
    Some(format!("{source_target}#^{block_id}{alias}"))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DependencyMetadataRepair {
    pub(super) contents: String,
    pub(super) count: usize,
}
pub(super) fn repair_dependency_metadata(
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
pub(super) fn repair_wiki_links(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> LinkRepairResult {
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
        if let Some(new_inner) = repair_wiki_link_inner(
            inner,
            current_path,
            note_index,
            moved_targets,
        ) {
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
pub(super) fn repair_wiki_link_inner(
    inner: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (target, fragment) = split_block_fragment(target_with_fragment)?;
    let resolved_path = note_index.resolve(current_path, target)?;
    let block_id = fragment.strip_prefix('^')?;
    let moved_target =
        moved_targets.get(&(resolved_path, block_id.to_string()))?;
    Some(format!(
        "{}#^{}{}",
        moved_target.archive_target, moved_target.block_id, alias
    ))
}
pub(super) fn repair_markdown_links(
    contents: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> LinkRepairResult {
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
        if let Some(new_destination) = repair_markdown_destination(
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
pub(super) fn next_wiki_link_start(
    contents: &str,
    cursor: usize,
) -> Option<usize> {
    contents[cursor..]
        .find("[[")
        .map(|relative_start| cursor + relative_start)
}
pub(super) fn repair_markdown_destination(
    destination: &str,
    current_path: &Path,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
) -> Option<String> {
    if !is_simple_markdown_destination(destination) {
        return None;
    }

    let (target, fragment) = split_block_fragment(destination)?;
    if target.contains("://") {
        return None;
    }

    let resolved_path = note_index.resolve(current_path, target)?;
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
pub(super) fn split_block_fragment(target: &str) -> Option<(&str, &str)> {
    let (note_target, fragment) = target.split_once('#')?;
    is_block_fragment(fragment).then_some((note_target, fragment))
}
pub(super) fn is_block_fragment(fragment: &str) -> bool {
    let Some(block_id) = fragment.strip_prefix('^') else {
        return false;
    };
    !block_id.is_empty() && block_id.bytes().all(is_block_id_byte)
}
pub(super) fn is_simple_markdown_destination(destination: &str) -> bool {
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
