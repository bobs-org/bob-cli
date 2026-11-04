//! Shared fragment transplant primitives used by task move, merge, and split.

use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::super::{
    collect_done::{
        self, block_id_occurrences_in_text, is_simple_markdown_destination,
        next_available_block_id, repair_dependency_metadata,
        repair_links_in_note_with, repair_markdown_destination,
        repair_wiki_link_inner, split_block_fragment, split_line_ending,
        MovedBlockTargets,
    },
    markdown,
    task_dependencies::inline_code_spans,
    vault_links::NoteIndex,
};

use super::plan::IdCollisionPolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenameRecord {
    pub(crate) path: PathBuf,
    pub(crate) old_block_id: String,
    pub(crate) new_block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepairedFile {
    pub(crate) path: PathBuf,
    pub(crate) link_count: usize,
    pub(crate) dependency_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RenameError {
    Collision {
        fragment_index: usize,
        block_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairedPostimages {
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) repaired_files: Vec<RepairedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransplantPlan {
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) fragments: Vec<String>,
    pub(crate) renames: Vec<RenameRecord>,
    pub(crate) repaired_files: Vec<RepairedFile>,
    pub(crate) landing_lines: Vec<usize>,
}

/// Cut an arbitrary inclusive-start, exclusive-end line range from a note.
pub(crate) fn cut_line_range(
    contents: &str,
    range: &LineRange,
) -> Option<String> {
    if range.start >= range.end {
        return None;
    }
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    (range.end <= lines.len()).then(|| lines[range.start..range.end].concat())
}

/// Remove sorted or unsorted line ranges, retaining the move engine's seam
/// cleanup so task moves keep their established output.
pub(crate) fn remove_line_ranges(
    contents: &str,
    ranges: &[LineRange],
) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let mut drop = vec![false; lines.len()];
    for range in ranges {
        for index in range.start..range.end.min(drop.len()) {
            drop[index] = true;
        }
    }
    let mut result = String::new();
    let mut last_emitted_blank = false;
    let mut at_seam = false;
    for (index, line) in lines.iter().enumerate() {
        if drop[index] {
            at_seam = true;
            continue;
        }
        let blank = split_line_ending(line).0.trim().is_empty();
        if at_seam && blank && last_emitted_blank {
            at_seam = false;
            continue;
        }
        at_seam = false;
        result.push_str(line);
        last_emitted_blank = blank;
    }
    result
}

/// Rename descendant anchors against the destination, reserving every
/// collision-free incoming ID before assigning deterministic suffixes.
pub(crate) fn rename_fragments(
    fragments: &[String],
    dest_path: &Path,
    dest_used: &BTreeSet<String>,
    collision_policy: IdCollisionPolicy,
) -> Result<
    (Vec<String>, Vec<RenameRecord>, BTreeMap<String, String>),
    RenameError,
> {
    let mut used = dest_used.clone();
    let mut reserved = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for fragment in fragments {
        for occurrence in fragment_occurrences(fragment) {
            if seen.insert(occurrence.id.clone())
                && !used.contains(&occurrence.id)
            {
                reserved.insert(occurrence.id);
            }
        }
    }

    let mut rewritten = Vec::with_capacity(fragments.len());
    let mut renames = Vec::new();
    let mut id_map = BTreeMap::new();
    for (fragment_index, fragment) in fragments.iter().enumerate() {
        let occurrences = fragment_occurrences(fragment);
        let mut finals = Vec::with_capacity(occurrences.len());
        for occurrence in &occurrences {
            let final_id = if used.contains(&occurrence.id) {
                if collision_policy == IdCollisionPolicy::Refuse {
                    return Err(RenameError::Collision {
                        fragment_index,
                        block_id: occurrence.id.clone(),
                    });
                }
                next_available_block_id(&occurrence.id, &used, &reserved)
            } else {
                occurrence.id.clone()
            };
            if final_id != occurrence.id {
                renames.push(RenameRecord {
                    path: dest_path.to_path_buf(),
                    old_block_id: occurrence.id.clone(),
                    new_block_id: final_id.clone(),
                });
            }
            used.insert(final_id.clone());
            id_map.insert(occurrence.id.clone(), final_id.clone());
            finals.push(final_id);
        }
        let mut updated = fragment.clone();
        for (occurrence, final_id) in occurrences.iter().zip(&finals).rev() {
            if final_id != &occurrence.id {
                updated
                    .replace_range(occurrence.start..occurrence.end, final_id);
            }
        }
        rewritten.push(updated);
    }
    Ok((rewritten, renames, id_map))
}

/// Rebase list indentation while retaining the bytes inside fenced code,
/// apart from replacing the fragment's outer indentation prefix.
pub(crate) fn rebase_fragment(
    fragment: &str,
    source_indent: &str,
    destination_indent: &str,
    source_unit: &str,
    destination_unit: &str,
    convert_units: bool,
) -> String {
    if source_indent == destination_indent && !convert_units {
        return fragment.to_string();
    }
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let fenced = fragment_fenced_lines(&logical, source_indent);
    let mut result = String::with_capacity(fragment.len());
    for (index, line) in lines.iter().enumerate() {
        let (content, ending) = split_line_ending(line);
        if content.trim().is_empty() {
            result.push_str(content);
            result.push_str(ending);
            continue;
        }
        let indent_len = content
            .find(|character: char| !matches!(character, ' ' | '\t'))
            .unwrap_or(content.len());
        let indent = &content[..indent_len];
        let relative = indent.strip_prefix(source_indent).unwrap_or(indent);
        let relative = if convert_units && !fenced.contains(&index) {
            convert_indent(relative, source_unit, destination_unit)
        } else {
            relative.to_string()
        };
        result.push_str(destination_indent);
        result.push_str(&relative);
        result.push_str(&content[indent_len..]);
        result.push_str(ending);
    }
    result
}

fn fragment_fenced_lines(
    logical: &[&str],
    source_indent: &str,
) -> BTreeSet<usize> {
    let normalized: Vec<String> = logical
        .iter()
        .map(|line| {
            let indent_len = line
                .find(|character: char| !matches!(character, ' ' | '\t'))
                .unwrap_or(line.len());
            let indent = &line[..indent_len];
            let relative = indent.strip_prefix(source_indent).unwrap_or(indent);
            format!("{relative}{}", &line[indent_len..])
        })
        .collect();
    let normalized_refs: Vec<&str> =
        normalized.iter().map(String::as_str).collect();
    let mut fenced =
        markdown::fenced_lines(&normalized_refs, 0..normalized_refs.len());
    let mut marker = None;
    for (index, line) in normalized.iter().enumerate() {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if let Some(open) = marker {
            fenced.insert(index);
            if closes_fragment_fence(trimmed, open) {
                marker = None;
            }
        } else if let Some(open) = fragment_fence_marker(trimmed) {
            fenced.insert(index);
            marker = Some(open);
        }
    }
    fenced
}

fn fragment_fence_marker(line: &str) -> Option<char> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = line
        .chars()
        .take_while(|character| *character == marker)
        .count();
    (count >= 3).then_some(marker)
}

fn closes_fragment_fence(line: &str, marker: char) -> bool {
    let count = line
        .chars()
        .take_while(|character| *character == marker)
        .count();
    count >= 3 && line[count..].trim().is_empty()
}

fn convert_indent(
    indent: &str,
    source_unit: &str,
    destination_unit: &str,
) -> String {
    if source_unit.is_empty() || destination_unit.is_empty() {
        return indent.to_string();
    }
    let mut remaining = indent;
    let mut converted = String::with_capacity(indent.len());
    while let Some(rest) = remaining.strip_prefix(source_unit) {
        converted.push_str(destination_unit);
        remaining = rest;
    }
    converted.push_str(remaining);
    converted
}

/// Insert already-rebased fragments at a byte offset and report each
/// fragment's zero-based landing line.
pub(crate) fn insert_fragments_at_byte(
    contents: &str,
    offset: usize,
    fragments: &[String],
    extra_separator_after_heading: bool,
) -> Option<(String, Vec<usize>)> {
    if offset > contents.len() || !contents.is_char_boundary(offset) {
        return None;
    }
    let insertion = join_fragments(fragments, document_line_ending(contents));
    let (postimage, first_line) = insert_at_byte_with_line(
        contents,
        offset,
        &insertion,
        extra_separator_after_heading,
    );
    Some((postimage, fragment_landing_lines(first_line, fragments)))
}

/// Apply link repair to cut-source postimages and every other affected note.
/// The destination has already been link-repaired before insertion so its
/// line-based/slot insertion policy remains stable.
pub(crate) fn repair_vault_postimages(
    originals: &BTreeMap<PathBuf, String>,
    source_postimages: &BTreeMap<PathBuf, String>,
    destination: &Path,
    destination_postimage: String,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    mut repair_counts: BTreeMap<PathBuf, (usize, usize)>,
) -> RepairedPostimages {
    let mut postimages = source_postimages.clone();
    postimages.insert(destination.to_path_buf(), destination_postimage);
    let source_paths: BTreeSet<PathBuf> =
        source_postimages.keys().cloned().collect();

    for (path, contents) in source_postimages {
        if path == destination {
            continue;
        }
        let repair = repair_links_in_note_with(
            contents,
            path,
            note_index,
            moved_targets,
            true,
        );
        add_repair(
            &mut repair_counts,
            path.clone(),
            repair.link_count,
            repair.dependency_metadata_count,
        );
        postimages.insert(path.clone(), repair.contents);
    }

    for (path, original) in originals {
        if path == destination || source_paths.contains(path) {
            continue;
        }
        let repair = repair_links_in_note_with(
            original,
            path,
            note_index,
            moved_targets,
            true,
        );
        if repair.link_count > 0 || repair.dependency_metadata_count > 0 {
            add_repair(
                &mut repair_counts,
                path.clone(),
                repair.link_count,
                repair.dependency_metadata_count,
            );
            postimages.insert(path.clone(), repair.contents);
        }
    }

    postimages.retain(|path, contents| {
        originals.get(path).map(String::as_str) != Some(contents.as_str())
    });
    let repaired_files = repair_counts
        .into_iter()
        .filter(|(_, (links, deps))| *links > 0 || *deps > 0)
        .map(
            |(path, (link_count, dependency_token_count))| RepairedFile {
                path,
                link_count,
                dependency_token_count,
            },
        )
        .collect();
    RepairedPostimages {
        postimages,
        repaired_files,
    }
}

/// Collect the completed transplant result after destination insertion and
/// vault-wide repair have both been planned.
pub(crate) fn finish_transplant(
    repaired: RepairedPostimages,
    fragments: Vec<String>,
    renames: Vec<RenameRecord>,
    landing_lines: Vec<usize>,
) -> TransplantPlan {
    TransplantPlan {
        postimages: repaired.postimages,
        fragments,
        renames,
        repaired_files: repaired.repaired_files,
        landing_lines,
    }
}

pub(crate) fn live_block_ids(contents: &str) -> Vec<String> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let hidden = hidden_markdown_indexes(&logical);
    logical
        .iter()
        .enumerate()
        .filter(|(index, _)| !hidden.contains(index))
        .filter_map(|(_, line)| collect_done::trailing_block_id_in_line(line))
        .collect()
}

fn hidden_markdown_indexes(logical: &[&str]) -> BTreeSet<usize> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let mut hidden = markdown::fenced_lines(logical, scan_start..logical.len());
    if let Some(end) = frontmatter_end {
        hidden.extend(0..=end);
    }
    hidden
}

fn add_repair(
    counts: &mut BTreeMap<PathBuf, (usize, usize)>,
    path: PathBuf,
    links: usize,
    deps: usize,
) {
    let entry = counts.entry(path).or_default();
    entry.0 += links;
    entry.1 += deps;
}

pub(crate) fn document_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

pub(crate) fn join_fragments(fragments: &[String], ending: &str) -> String {
    let mut joined = String::new();
    for fragment in fragments {
        if !joined.is_empty() && !joined.ends_with('\n') {
            joined.push_str(ending);
        }
        joined.push_str(fragment);
    }
    if !joined.is_empty() && !joined.ends_with('\n') {
        joined.push_str(ending);
    }
    joined
}

fn insert_at_byte_with_line(
    contents: &str,
    index: usize,
    insertion: &str,
    extra_separator_after_heading: bool,
) -> (String, usize) {
    let ending = document_line_ending(contents);
    let mut addition = insertion.to_string();
    let needs_leading = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading {
        addition = format!("{ending}{addition}");
    }
    if index == 0 || contents[..index].ends_with(&format!("{ending}{ending}")) {
        // Already separated by a blank line.
    } else if extra_separator_after_heading
        || last_line_is_heading(&contents[..index])
    {
        addition = format!("{ending}{addition}");
    }
    let leading_len = addition.len().saturating_sub(insertion.len());
    let insertion_line = contents[..index].matches('\n').count()
        + addition[..leading_len].matches('\n').count();
    let mut result = String::with_capacity(contents.len() + addition.len());
    result.push_str(&contents[..index]);
    result.push_str(&addition);
    result.push_str(&contents[index..]);
    (result, insertion_line)
}

fn last_line_is_heading(prefix: &str) -> bool {
    let last = prefix
        .rsplit_once('\n')
        .map(|(_, last)| split_line_ending(last).0)
        .unwrap_or_else(|| split_line_ending(prefix).0);
    let last = if last.is_empty() {
        prefix
            .rsplit('\n')
            .nth(1)
            .map(|line| split_line_ending(line).0)
            .unwrap_or("")
    } else {
        last
    };
    markdown::atx_heading(last).is_some()
}

pub(crate) fn fragment_landing_lines(
    first_line: usize,
    fragments: &[String],
) -> Vec<usize> {
    let mut next = first_line;
    fragments
        .iter()
        .map(|fragment| {
            let line = next;
            next += fragment.lines().count().max(1);
            line
        })
        .collect()
}

pub(crate) struct Occurrence {
    pub(crate) id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn fragment_occurrences(fragment: &str) -> Vec<Occurrence> {
    let lines: Vec<&str> = fragment.split_inclusive('\n').collect();
    let logical: Vec<&str> =
        lines.iter().map(|line| split_line_ending(line).0).collect();
    let hidden = hidden_markdown_indexes(&logical);
    let mut occurrences = Vec::new();
    let mut offset = 0;
    for (index, line) in lines.iter().enumerate() {
        let (content, _) = split_line_ending(line);
        if !hidden.contains(&index) {
            for occurrence in block_id_occurrences_in_text(content) {
                occurrences.push(Occurrence {
                    id: occurrence.id,
                    start: offset + occurrence.start,
                    end: offset + occurrence.end,
                });
            }
        }
        offset += line.len();
    }
    occurrences
}

pub(crate) struct FragmentRepair {
    pub(crate) contents: String,
    pub(crate) link_count: usize,
    pub(crate) dependency_count: usize,
}

pub(crate) fn repair_moved_fragment(
    fragment: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> FragmentRepair {
    let wiki = repair_fragment_wiki_links(
        fragment,
        source_path,
        source_target,
        note_index,
        moved_targets,
        stayed_source_ids,
        id_map,
    );
    let markdown = repair_fragment_markdown_links(
        &wiki.contents,
        source_path,
        source_target,
        note_index,
        moved_targets,
        stayed_source_ids,
        id_map,
    );
    let metadata =
        repair_dependency_metadata(&markdown.contents, moved_targets);
    FragmentRepair {
        contents: metadata.contents,
        link_count: wiki.link_count + markdown.link_count,
        dependency_count: metadata.count,
    }
}

fn repair_fragment_wiki_links(
    contents: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> collect_done::LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
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
        let code_spans = inline_code_spans(line);
        let relative_in_line = start.saturating_sub(line_start);
        let skip = fenced.contains(&line_index)
            || code_spans.iter().any(|span| {
                relative_in_line < span.end && end + 2 - line_start > span.start
            });
        let replacement = if skip {
            None
        } else {
            repair_fragment_wiki_inner(
                inner,
                source_path,
                source_target,
                note_index,
                moved_targets,
                stayed_source_ids,
                id_map,
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
    collect_done::LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn repair_fragment_wiki_inner(
    inner: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> Option<String> {
    let (target_with_fragment, alias) = match inner.find('|') {
        Some(index) => (&inner[..index], &inner[index..]),
        None => (inner, ""),
    };
    let (target, fragment) = split_block_fragment(target_with_fragment)?;
    let block_id = fragment.strip_prefix('^')?;
    if target.trim().is_empty() {
        if let Some(final_id) = id_map.get(block_id) {
            return Some(format!("#^{final_id}{alias}"));
        }
        if stayed_source_ids.contains(block_id) {
            return Some(format!("{source_target}#^{block_id}{alias}"));
        }
        return None;
    }
    repair_wiki_link_inner(inner, source_path, note_index, moved_targets)
}

fn repair_fragment_markdown_links(
    contents: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> collect_done::LinkRepairResult {
    let lines: Vec<&str> = contents
        .split_inclusive('\n')
        .map(|line| split_line_ending(line).0)
        .collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut repaired = String::with_capacity(contents.len());
    let mut link_count = 0;
    let mut cursor = 0;
    while let Some(relative_open) = contents[cursor..].find("](") {
        let open = cursor + relative_open;
        if let Some(wiki_start) =
            contents[cursor..].find("[[").map(|rel| cursor + rel)
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
        let code_spans = inline_code_spans(line);
        let relative_in_line = open.saturating_sub(line_start);
        let skip = fenced.contains(&line_index)
            || code_spans.iter().any(|span| {
                relative_in_line < span.end
                    && destination_end + 1 - line_start > span.start
            });
        if skip {
            repaired.push_str(destination);
        } else if let Some(new_destination) =
            repair_fragment_markdown_destination(
                destination,
                source_path,
                source_target,
                note_index,
                moved_targets,
                stayed_source_ids,
                id_map,
            )
        {
            repaired.push_str(&new_destination);
            link_count += 1;
        } else {
            repaired.push_str(destination);
        }
        repaired.push(')');
        cursor = destination_end + 1;
    }
    repaired.push_str(&contents[cursor..]);
    collect_done::LinkRepairResult {
        contents: repaired,
        link_count,
        dependency_metadata_count: 0,
    }
}

fn repair_fragment_markdown_destination(
    destination: &str,
    source_path: &Path,
    source_target: &str,
    note_index: &NoteIndex,
    moved_targets: &MovedBlockTargets,
    stayed_source_ids: &BTreeSet<String>,
    id_map: &BTreeMap<String, String>,
) -> Option<String> {
    if !is_simple_markdown_destination(destination) {
        return None;
    }
    let (target, fragment) = split_block_fragment(destination)?;
    if target.contains("://") {
        return None;
    }
    let block_id = fragment.strip_prefix('^')?;
    if target.is_empty() {
        if let Some(final_id) = id_map.get(block_id) {
            return Some(format!("#^{final_id}"));
        }
        if stayed_source_ids.contains(block_id) {
            return Some(format!("{source_target}.md#^{block_id}"));
        }
        return None;
    }
    repair_markdown_destination(
        destination,
        source_path,
        note_index,
        moved_targets,
    )
}

#[cfg(test)]
mod tests;
