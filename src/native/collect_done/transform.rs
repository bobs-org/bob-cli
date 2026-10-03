//! Markdown collection and block-id scanning.
use super::super::is_always_excluded_note_directory_name;
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Transform {
    pub(super) task_count: usize,
    pub(super) source_contents: String,
    pub(super) archive_append: String,
    pub(super) moved_block_ids: BTreeSet<String>,
    pub(super) ambiguous_moved_block_ids: BTreeSet<String>,
    pub(super) moved_block_id_occurrences: Vec<BlockIdOccurrence>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockIdOccurrence {
    pub(super) id: String,
    pub(super) start: usize,
    pub(super) end: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskLine {
    pub(super) indent: usize,
}
pub(super) fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, vault, false, &mut files)?;
    files.sort();
    Ok(files)
}
pub(super) fn link_repair_markdown_files(
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
pub(super) fn transform_markdown(contents: &str) -> Transform {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let mut source_contents = String::with_capacity(contents.len());
    let mut archive_append = String::new();
    let mut moved_block_ids = BTreeSet::new();
    let mut ambiguous_moved_block_ids = BTreeSet::new();
    let mut moved_block_id_occurrences = Vec::new();
    let mut task_count = 0;
    let mut index = 0;

    while index < lines.len() {
        let Some(task_line) = collectible_task_line(lines[index]) else {
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        };

        let end = task_block_end(&lines, index, task_line.indent);
        task_count += 1;
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
        index = end;
    }

    Transform {
        task_count,
        source_contents,
        archive_append,
        moved_block_ids,
        ambiguous_moved_block_ids,
        moved_block_id_occurrences,
    }
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
pub(super) fn block_id_occurrences_in_text(
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
pub(super) fn task_block_end(
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
pub(super) fn collectible_task_line(line: &str) -> Option<TaskLine> {
    let (content, _) = split_line_ending(line);
    let indent = leading_indent_len(content);
    let rest = &content[indent..];
    let rest = strip_list_marker(rest)?.trim_start();
    let checkbox = rest.get(..3)?;

    if !matches!(checkbox, "[x]" | "[X]" | "[-]") {
        return None;
    }

    let after_checkbox = &rest[3..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return None;
    }

    has_task_tag(content).then_some(TaskLine { indent })
}
pub(super) fn strip_list_marker(line: &str) -> Option<&str> {
    let first = line.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        let after_marker = &line[first.len_utf8()..];
        if after_marker.starts_with(char::is_whitespace) {
            return Some(after_marker);
        }
    }

    let digit_len = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return None;
    }

    let after_digits = &line[digit_len..];
    let marker = after_digits.chars().next()?;
    if !matches!(marker, '.' | ')') {
        return None;
    }

    let after_marker = &after_digits[marker.len_utf8()..];
    after_marker
        .starts_with(char::is_whitespace)
        .then_some(after_marker)
}
pub(super) fn has_task_tag(text: &str) -> bool {
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
pub(super) fn leading_indent_len(line: &str) -> usize {
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
