//! Markdown collection and block-id scanning.
use super::super::is_always_excluded_note_directory_name;
use super::*;

pub(super) const SKIP_DAILY: &str = "daily";
pub(super) const SKIP_NOT_AREA_OR_PROJECT: &str = "not_area_or_project";
pub(super) const SKIP_FINISHED_PROJECT: &str = "finished_project";
pub(super) const SKIP_OPEN_PARENT: &str = "open_parent";
pub(super) const SKIP_PRJ: &str = "prj";
pub(super) const HOLD_OPEN_SUBTASK: &str = "open_subtask";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Transform {
    pub(super) task_count: usize,
    pub(super) source_contents: String,
    pub(super) archive_append: String,
    pub(super) moved_block_ids: BTreeSet<String>,
    pub(super) ambiguous_moved_block_ids: BTreeSet<String>,
    pub(super) moved_block_id_occurrences: Vec<BlockIdOccurrence>,
    pub(super) moved_roots: Vec<MovedRootSpan>,
    pub(super) holds: Vec<HeldTask>,
    pub(super) skip_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HeldTask {
    pub(super) line: usize,
    pub(super) reason: &'static str,
    pub(super) block_ids: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockIdOccurrence {
    pub(crate) id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// One selected archive root with its source position and byte span.
///
/// `start`/`end` are byte offsets into the transform's `archive_append`;
/// `root_id` is the trailing block id on the root line when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MovedRootSpan {
    pub(super) source_line: usize,
    pub(super) root_id: Option<String>,
    pub(super) start: usize,
    pub(super) end: usize,
}
pub(super) fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = super::super::task_status_hooks::markdown_files(vault)?;
    files.sort();
    Ok(files)
}
pub(crate) fn link_repair_markdown_files(
    vault: &Path,
) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, vault, true, &mut files)?;
    files.sort();
    Ok(files)
}
pub(super) fn collect_markdown_files(
    vault: &Path,
    directory: &Path,
    include_done: bool,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if should_skip_directory(vault, &path, include_done) {
                continue;
            }
            collect_markdown_files(vault, &path, include_done, files)?;
        } else if file_type.is_file() && is_markdown_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}
pub(super) fn should_skip_directory(
    vault: &Path,
    directory: &Path,
    include_done: bool,
) -> bool {
    let relative = directory.strip_prefix(vault).unwrap_or(directory);
    relative.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if (!include_done && name == OsStr::new("done"))
                    || name.to_str().is_some_and(|name| name.starts_with('.'))
                    || is_always_excluded_note_directory_name(name)
        )
    })
}
pub(super) fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}
pub(super) fn archive_relative_path(
    source_relative_path: &Path,
) -> io::Result<PathBuf> {
    let stem = source_relative_path.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "source path has no file stem: {}",
                source_relative_path.display()
            ),
        )
    })?;
    let mut archive_name = OsString::from(stem);
    archive_name.push("_done.md");

    let mut archive_path = PathBuf::from("done");
    if let Some(parent) = source_relative_path.parent()
        && !parent.as_os_str().is_empty()
    {
        archive_path.push(parent);
    }
    archive_path.push(archive_name);
    Ok(archive_path)
}
pub(super) fn archive_wiki_link(
    archive_relative_path: &Path,
) -> io::Result<String> {
    vault_relative_wiki_link(archive_relative_path, "archive")
}
pub(super) fn source_wiki_link(
    source_relative_path: &Path,
) -> io::Result<String> {
    vault_relative_wiki_link(source_relative_path, "source")
}
pub(super) fn vault_relative_wiki_link(
    relative_path: &Path,
    path_kind: &str,
) -> io::Result<String> {
    Ok(format!(
        "[[{}]]",
        vault_relative_link_target(relative_path, path_kind)?
    ))
}
pub(crate) fn vault_relative_link_target(
    relative_path: &Path,
    path_kind: &str,
) -> io::Result<String> {
    super::super::task_dependencies::vault_relative_link_target_with_kind(
        relative_path,
        path_kind,
    )
}
pub(super) use super::super::task_dependencies::dependency_id;
#[cfg(test)]
pub(super) fn transform_markdown(contents: &str) -> Transform {
    transform_markdown_with_settings(contents, &default_archive_settings())
}

#[cfg(test)]
pub(super) fn default_archive_settings(
) -> super::super::task_status_hooks::TasksSettings {
    super::super::task_status_hooks::read_tasks_settings(Path::new(""))
}

pub(super) fn transform_markdown_with_settings(
    contents: &str,
    settings: &super::super::task_status_hooks::TasksSettings,
) -> Transform {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let masked = super::super::markdown::non_content_line_indexes(&lines);
    let parsed =
        super::super::task_status_hooks::parse_tasks(contents, settings);
    let mut open_lines = BTreeSet::new();
    let mut closed_lines = BTreeSet::new();
    for task in &parsed {
        let line = lines
            .get(task.line_index)
            .map(|line| split_line_ending(line).0)
            .unwrap_or_default();
        if !has_task_tag(line) {
            continue;
        }
        if task.status_type.is_open() {
            open_lines.insert(task.line_index);
        } else if is_closed_status(task.status_type) {
            closed_lines.insert(task.line_index);
        }
    }
    let mut source_contents = String::with_capacity(contents.len());
    let mut archive_append = String::new();
    let mut moved_block_ids = BTreeSet::new();
    let mut ambiguous_moved_block_ids = BTreeSet::new();
    let mut moved_block_id_occurrences = Vec::new();
    let mut moved_roots = Vec::new();
    let mut holds = Vec::new();
    let mut skip_counts = BTreeMap::new();
    let mut open_task_indents: Vec<usize> = Vec::new();
    let mut task_count = 0;
    let mut index = 0;

    while index < lines.len() {
        let (content, _) = split_line_ending(lines[index]);
        if content.trim().is_empty() || masked.contains(&index) {
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        }

        let indent = leading_indent_len(content);
        while open_task_indents
            .last()
            .is_some_and(|&open_indent| open_indent >= indent)
        {
            open_task_indents.pop();
        }

        if open_lines.contains(&index) {
            open_task_indents.push(indent);
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        }

        if !closed_lines.contains(&index) {
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        }

        if super::super::projects::has_trailing_prj_anchor(content) {
            bump_skip(&mut skip_counts, SKIP_PRJ);
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        }

        if !open_task_indents.is_empty() {
            bump_skip(&mut skip_counts, SKIP_OPEN_PARENT);
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        }

        let end = task_block_end(&lines, index, indent);
        if block_has_open_task(index, end, &open_lines, &masked) {
            holds.push(HeldTask {
                line: index + 1,
                reason: HOLD_OPEN_SUBTASK,
                block_ids: held_block_ids(&lines[index..end]),
            });
            for line in &lines[index..end] {
                source_contents.push_str(line);
            }
            index = end;
            continue;
        }

        task_count += 1;
        let (root_content, _) = split_line_ending(lines[index]);
        let root_id = trailing_block_id_in_line(root_content);
        let span_start = archive_append.len();
        for line in &lines[index..end] {
            let line_offset = archive_append.len();
            archive_append.push_str(line);
            let (content, _) = split_line_ending(line);
            for occurrence in block_id_occurrences_in_text(content) {
                moved_block_id_occurrences.push(BlockIdOccurrence {
                    id: occurrence.id.clone(),
                    start: line_offset + occurrence.start,
                    end: line_offset + occurrence.end,
                });
                let block_id = occurrence.id;
                if !moved_block_ids.insert(block_id.clone()) {
                    ambiguous_moved_block_ids.insert(block_id);
                }
            }
        }
        moved_roots.push(MovedRootSpan {
            source_line: index + 1,
            root_id,
            start: span_start,
            end: archive_append.len(),
        });
        index = end;
    }

    Transform {
        task_count,
        source_contents,
        archive_append,
        moved_block_ids,
        ambiguous_moved_block_ids,
        moved_block_id_occurrences,
        moved_roots,
        holds,
        skip_counts,
    }
}

/// Block ids carried by a held root block, root first, in line order.
fn held_block_ids(span: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for line in span {
        let (content, _) = split_line_ending(line);
        ids.extend(block_ids_in_text(content));
    }
    ids
}

pub(super) fn is_closed_status(
    status_type: super::super::task_status_hooks::TaskStatusType,
) -> bool {
    matches!(
        status_type,
        super::super::task_status_hooks::TaskStatusType::Done
            | super::super::task_status_hooks::TaskStatusType::Cancelled
    )
}

fn bump_skip(counts: &mut BTreeMap<String, usize>, reason: &str) {
    *counts.entry(reason.to_string()).or_insert(0) += 1;
}

fn block_has_open_task(
    start: usize,
    end: usize,
    open_lines: &BTreeSet<usize>,
    masked: &BTreeSet<usize>,
) -> bool {
    ((start + 1)..end)
        .any(|index| !masked.contains(&index) && open_lines.contains(&index))
}
pub(crate) fn block_ids_in_markdown(contents: &str) -> Vec<String> {
    let mut block_ids = Vec::new();
    for line in contents.split_inclusive('\n') {
        let (content, _) = split_line_ending(line);
        block_ids.extend(block_ids_in_text(content));
    }
    block_ids
}
pub(super) fn block_ids_in_text(text: &str) -> Vec<String> {
    trailing_block_id_in_line(text).into_iter().collect()
}
pub(crate) fn trailing_block_id_in_line(text: &str) -> Option<String> {
    block_id_occurrences_in_text(text)
        .into_iter()
        .next()
        .map(|occurrence| occurrence.id)
}
pub(crate) fn block_id_occurrences_in_text(
    text: &str,
) -> Vec<BlockIdOccurrence> {
    let trimmed = text.trim_end();
    let Some(start) = trimmed.rfind('^') else {
        return Vec::new();
    };
    let id_start = start + 1;
    if id_start == trimmed.len()
        || !trimmed[id_start..].bytes().all(is_block_id_byte)
        || (start > 0
            && !trimmed[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
    {
        return Vec::new();
    }
    vec![BlockIdOccurrence {
        id: trimmed[id_start..].to_string(),
        start: id_start,
        end: trimmed.len(),
    }]
}
pub(crate) fn is_block_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}
pub(crate) fn task_block_end(
    lines: &[&str],
    start: usize,
    task_indent: usize,
) -> usize {
    let mut index = start + 1;
    let mut include_end = start + 1;
    let mut pending_blank = false;

    while index < lines.len() {
        let (content, _) = split_line_ending(lines[index]);
        if content.trim().is_empty() {
            pending_blank = true;
            index += 1;
            continue;
        }

        if leading_indent_len(content) > task_indent {
            pending_blank = false;
            index += 1;
            include_end = index;
            continue;
        }

        break;
    }

    if pending_blank && index == lines.len() {
        include_end = index;
    }

    include_end
}
pub(crate) fn has_task_tag(text: &str) -> bool {
    let mut rest = text;
    while let Some(index) = rest.find("#task") {
        let after_index = index + "#task".len();
        let after = rest[after_index..].chars().next();
        if after.map(is_task_tag_boundary).unwrap_or(true) {
            return true;
        }
        rest = &rest[after_index..];
    }

    false
}
pub(super) fn is_task_tag_boundary(character: char) -> bool {
    !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
}
pub(crate) fn leading_indent_len(line: &str) -> usize {
    line.char_indices()
        .find_map(|(index, character)| {
            (!matches!(character, ' ' | '\t')).then_some(index)
        })
        .unwrap_or(line.len())
}
pub(crate) fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        return (content, "\r\n");
    }
    if let Some(content) = line.strip_suffix('\n') {
        return (content, "\n");
    }
    (line, "")
}
