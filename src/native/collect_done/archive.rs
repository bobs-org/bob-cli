//! Archive frontmatter, appends, and atomic writes.
use super::*;

pub(super) const ARCHIVE_TYPE_LINE: &str = "type: \"[[done]]\"";
pub(super) const DONE_TASKS_KEY: &str = "done_tasks:";
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeduplicatedArchiveAppend {
    pub(super) archive_append: String,
    pub(super) final_block_ids: BTreeMap<String, String>,
    pub(super) rename_count: usize,
}
pub(super) fn read_optional_string(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fs_error("read", path, error)),
    }
}
pub(super) fn deduplicate_archive_append_block_ids(
    existing_archive: Option<&str>,
    archive_append: &str,
    occurrences: &[BlockIdOccurrence],
) -> DeduplicatedArchiveAppend {
    if occurrences.is_empty() {
        return DeduplicatedArchiveAppend {
            archive_append: archive_append.to_string(),
            final_block_ids: BTreeMap::new(),
            rename_count: 0,
        };
    }

    let mut used = BTreeSet::new();
    if let Some(existing_archive) = existing_archive {
        used.extend(block_ids_in_markdown(existing_archive));
    }

    let mut occurrence_counts = BTreeMap::new();
    for occurrence in occurrences {
        *occurrence_counts.entry(occurrence.id.clone()).or_insert(0) += 1;
    }

    let mut reserved_unchanged_ids = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    for occurrence in occurrences {
        if seen_ids.insert(occurrence.id.clone())
            && !used.contains(&occurrence.id)
        {
            reserved_unchanged_ids.insert(occurrence.id.clone());
        }
    }

    let mut final_ids = Vec::with_capacity(occurrences.len());
    let mut final_block_ids = BTreeMap::new();
    let mut rename_count = 0;
    for occurrence in occurrences {
        let final_id = if used.contains(&occurrence.id) {
            next_available_block_id(
                &occurrence.id,
                &used,
                &reserved_unchanged_ids,
            )
        } else {
            occurrence.id.clone()
        };

        if final_id != occurrence.id {
            rename_count += 1;
        }
        used.insert(final_id.clone());
        if occurrence_counts.get(&occurrence.id) == Some(&1) {
            final_block_ids.insert(occurrence.id.clone(), final_id.clone());
        }
        final_ids.push(final_id);
    }

    let mut deduplicated = archive_append.to_string();
    for (occurrence, final_id) in occurrences.iter().zip(final_ids.iter()).rev()
    {
        if final_id != &occurrence.id {
            deduplicated
                .replace_range(occurrence.start..occurrence.end, final_id);
        }
    }

    DeduplicatedArchiveAppend {
        archive_append: deduplicated,
        final_block_ids,
        rename_count,
    }
}
pub(super) fn next_available_block_id(
    original_id: &str,
    used: &BTreeSet<String>,
    reserved_unchanged_ids: &BTreeSet<String>,
) -> String {
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{original_id}-{suffix}");
        if !used.contains(&candidate)
            && !reserved_unchanged_ids.contains(&candidate)
        {
            return candidate;
        }
        suffix += 1;
    }
}
pub(super) fn archive_contents(
    existing_archive: Option<&str>,
    archive_append: &str,
    source_link: &str,
) -> String {
    let mut contents =
        archive_base_contents(existing_archive, archive_append, source_link);
    append_archive_blocks(&mut contents, archive_append);
    contents
}
pub(super) fn archive_base_contents(
    existing_archive: Option<&str>,
    sample: &str,
    source_link: &str,
) -> String {
    match existing_archive {
        Some(contents) => ensure_archive_frontmatter(contents, source_link),
        None => archive_frontmatter(sample, source_link),
    }
}
pub(super) fn ensure_archive_frontmatter(
    contents: &str,
    source_link: &str,
) -> String {
    let newline = preferred_line_ending(contents);
    let parent_line = archive_parent_frontmatter_line(source_link);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    if lines
        .first()
        .map(|line| is_frontmatter_marker(split_line_ending(line).0))
        != Some(true)
    {
        let mut with_frontmatter = archive_frontmatter(contents, source_link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    }

    let Some(closing_index) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            is_frontmatter_marker(split_line_ending(line).0).then_some(index)
        })
    else {
        let mut with_frontmatter = archive_frontmatter(contents, source_link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    };

    let mut result = String::with_capacity(contents.len() + 64);

    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            result.push_str(line);
        } else if index < closing_index {
            let (content, _) = split_line_ending(line);
            if !is_parent_frontmatter_line(content)
                && !is_type_frontmatter_line(content)
            {
                result.push_str(line);
            }
        } else if index == closing_index {
            result.push_str(&parent_line);
            result.push_str(newline);
            result.push_str(ARCHIVE_TYPE_LINE);
            result.push_str(newline);
            result.push_str(line);
        } else {
            result.push_str(line);
        }
    }

    result
}
pub(super) fn archive_frontmatter(sample: &str, source_link: &str) -> String {
    let newline = preferred_line_ending(sample);
    let parent_line = archive_parent_frontmatter_line(source_link);
    format!(
        "---{newline}{parent_line}{newline}{ARCHIVE_TYPE_LINE}{newline}---{newline}{newline}"
    )
}
pub(super) fn archive_parent_frontmatter_line(source_link: &str) -> String {
    format!("parent: \"{source_link}\"")
}
pub(super) fn ensure_source_done_tasks_frontmatter(
    contents: &str,
    link: &str,
) -> String {
    let newline = preferred_line_ending(contents);
    let done_tasks_line = done_tasks_frontmatter_line(link);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    if lines
        .first()
        .map(|line| is_frontmatter_marker(split_line_ending(line).0))
        != Some(true)
    {
        let mut with_frontmatter =
            source_done_tasks_frontmatter(contents, link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    }

    let Some(closing_index) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            is_frontmatter_marker(split_line_ending(line).0).then_some(index)
        })
    else {
        let mut with_frontmatter =
            source_done_tasks_frontmatter(contents, link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    };

    let mut result = String::with_capacity(contents.len() + 80);
    let mut done_tasks_written = false;

    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            result.push_str(line);
        } else if index < closing_index {
            let (content, ending) = split_line_ending(line);
            if is_done_tasks_frontmatter_line(content) {
                result.push_str(&done_tasks_line);
                result.push_str(if ending.is_empty() {
                    newline
                } else {
                    ending
                });
                done_tasks_written = true;
            } else {
                result.push_str(line);
            }
        } else if index == closing_index {
            if !done_tasks_written {
                result.push_str(&done_tasks_line);
                result.push_str(newline);
            }
            result.push_str(line);
        } else {
            result.push_str(line);
        }
    }

    result
}
pub(super) fn source_done_tasks_frontmatter(
    sample: &str,
    link: &str,
) -> String {
    let newline = preferred_line_ending(sample);
    let done_tasks_line = done_tasks_frontmatter_line(link);
    format!("---{newline}{done_tasks_line}{newline}---{newline}{newline}")
}
pub(super) fn done_tasks_frontmatter_line(link: &str) -> String {
    format!("{DONE_TASKS_KEY} \"{link}\"")
}
pub(super) fn append_archive_blocks(
    contents: &mut String,
    archive_append: &str,
) {
    if archive_append.is_empty() {
        return;
    }

    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(archive_append);
}
pub(super) fn preferred_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}
pub(super) fn is_frontmatter_marker(content: &str) -> bool {
    content.trim() == "---"
}
pub(super) fn is_parent_frontmatter_line(content: &str) -> bool {
    content.trim_start().starts_with("parent:")
}
pub(super) fn is_type_frontmatter_line(content: &str) -> bool {
    content.trim_start().starts_with("type:")
}
pub(super) fn is_done_tasks_frontmatter_line(content: &str) -> bool {
    content.starts_with(DONE_TASKS_KEY)
}
pub(crate) fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            fs_error("create parent directory", parent, error)
        })?;
    }

    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents)
        .map_err(|error| fs_error("write temporary file", &temp_path, error))?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        fs_error("install file", path, error)
    })?;
    Ok(())
}
pub(super) fn temporary_write_path(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;

    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    Ok(path.with_file_name(temp_name))
}
pub(super) fn fs_error(
    action: &str,
    path: &Path,
    error: io::Error,
) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{action} {}: {error}", path.display()),
    )
}
