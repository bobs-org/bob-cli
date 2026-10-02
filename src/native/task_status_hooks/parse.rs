//! Markdown walk and task-line parsing for status sync.
use super::*;

pub(crate) fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, &mut files)?;
    Ok(files)
}

pub(super) fn collect_markdown_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if should_skip_directory(&name) {
                continue;
            }
            collect_markdown_files(&path, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn should_skip_directory(name: &OsStr) -> bool {
    name == OsStr::new("done")
        || name.to_str().is_some_and(|name| name.starts_with('.'))
        || is_always_excluded_note_directory_name(name)
}

pub(super) fn parse_tasks(
    contents: &str,
    settings: &TasksSettings,
) -> Vec<TaskLine> {
    let mut tasks = Vec::new();
    let mut byte_start = 0;
    for (line_index, segment) in contents.split_inclusive('\n').enumerate() {
        let line = logical_line(segment);
        if let Some(mut task) = parse_task_line(line, settings) {
            task.line_index = line_index;
            task.status_byte_offset += byte_start;
            tasks.push(task);
        }
        byte_start += segment.len();
    }
    tasks
}

pub(super) fn parse_task_line(
    line: &str,
    settings: &TasksSettings,
) -> Option<TaskLine> {
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(line, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let close_offset = line[index + 1..].find(']')?;
    let close = index + 1 + close_offset;
    let status_text = &line[index + 1..close];
    let mut status_chars = status_text.chars();
    let status = status_chars.next()?;
    if status_chars.next().is_some() {
        return None;
    }
    let after_checkbox = &line[close + 1..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return None;
    }
    let body = after_checkbox.trim_start();
    if !body.contains(&settings.global_filter) {
        return None;
    }
    let block_id = trailing_block_id(body);
    let metadata = task_metadata(body, block_id.as_deref());
    let description =
        task_description(body, &settings.global_filter, block_id.as_deref());
    let configured_status = settings.status_types.get(&status).copied();
    Some(TaskLine {
        line_index: 0,
        status,
        status_byte_offset: index + 1,
        block_id,
        task_id: metadata.task_id,
        depends_on: metadata.depends_on,
        scheduled: metadata.scheduled,
        status_type: configured_status.unwrap_or(TaskStatusType::Todo),
        status_recognized: configured_status.is_some(),
        description,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskMetadata {
    pub(crate) task_id: Option<String>,
    pub(crate) depends_on: Vec<String>,
    pub(crate) scheduled: Option<NaiveDate>,
}

pub(crate) fn task_metadata(
    body: &str,
    block_id: Option<&str>,
) -> TaskMetadata {
    let mut metadata = TaskMetadata::default();
    let mut state = block_id
        .and_then(|block_id| {
            body.trim_end()
                .strip_suffix(&format!("^{block_id}"))
                .map(str::trim_end)
        })
        .unwrap_or(body);

    for _ in 0..20 {
        let Some((start, key, value)) = trailing_dataview_field(state) else {
            if let Some(start) = trailing_task_tag_start(state) {
                state = state[..start].trim_end();
                continue;
            }
            break;
        };
        let recognized = match key {
            "id" if valid_task_identity(value) => {
                metadata.task_id = Some(value.to_string());
                true
            }
            "dependsOn" => parse_task_dependencies(value)
                .map(|dependencies| metadata.depends_on = dependencies)
                .is_some(),
            "priority" => matches!(
                value,
                "highest" | "high" | "medium" | "low" | "lowest"
            ),
            "scheduled" => parse_task_date(value)
                .map(|scheduled| metadata.scheduled = Some(scheduled))
                .is_some(),
            "start" | "created" | "due" | "completion" | "cancelled" => {
                valid_task_date(value)
            }
            "repeat" => value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b',' | b' ' | b'!')
            }),
            "onCompletion" => {
                !value.is_empty()
                    && value.bytes().all(|byte| byte.is_ascii_alphabetic())
            }
            _ => false,
        };
        if !recognized {
            break;
        }
        state = state[..start].trim_end();
    }
    metadata
}

pub(super) fn trailing_task_tag_start(state: &str) -> Option<usize> {
    let trimmed = state.trim_end();
    let start = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let tag = &trimmed[start..];
    let value = tag.strip_prefix('#')?;
    (!value.is_empty()
        && !value.chars().any(|character| {
            character.is_whitespace()
                || "!@#$%^&*(),.?\":{}|<>".contains(character)
        }))
    .then_some(start)
}

pub(super) fn trailing_dataview_field(
    state: &str,
) -> Option<(usize, &str, &str)> {
    let mut end = state.trim_end().len();
    if state[..end].ends_with(',') {
        end -= 1;
        end = state[..end].trim_end().len();
    }
    let close = state[..end].chars().next_back()?;
    let open = match close {
        ']' => '[',
        ')' => '(',
        _ => return None,
    };
    let without_close = &state[..end - close.len_utf8()];
    let start = without_close.rfind(open)?;
    let inner = without_close[start + open.len_utf8()..].trim();
    let (key, value) = inner.split_once("::")?;
    (key == key.trim()).then_some((start, key, value.trim()))
}

pub(super) fn valid_task_identity(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

pub(super) fn parse_task_dependencies(value: &str) -> Option<Vec<String>> {
    if value.is_empty() || value.bytes().any(|byte| byte == b'\t') {
        return None;
    }
    value
        .split(',')
        .map(|part| part.trim_matches(' '))
        .map(|part| valid_task_identity(part).then(|| part.to_string()))
        .collect()
}

pub(super) fn valid_task_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7) || byte.is_ascii_digit()
        })
}

pub(super) fn parse_task_date(value: &str) -> Option<NaiveDate> {
    valid_task_date(value)
        .then(|| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .flatten()
}

pub(crate) fn after_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        return bytes
            .get(index + 1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(index + 1);
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(index + digits), Some(b'.' | b')')) {
        return None;
    }
    bytes
        .get(index + digits + 1)
        .is_some_and(u8::is_ascii_whitespace)
        .then_some(index + digits + 1)
}

pub(super) fn trailing_block_id(body: &str) -> Option<String> {
    let token = body.split_whitespace().next_back()?;
    let block_id = token.strip_prefix('^')?;
    (!block_id.is_empty()
        && block_id.bytes().all(collect_done::is_block_id_byte))
    .then(|| block_id.to_string())
}

pub(super) fn task_description(
    body: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let without_block = block_id
        .and_then(|block_id| {
            body.trim_end()
                .strip_suffix(&format!("^{block_id}"))
                .map(str::trim_end)
        })
        .unwrap_or(body);
    without_block
        .replacen(global_filter, "", 1)
        .trim()
        .to_string()
}

pub(super) fn task_blocks(
    files: &[FileScan],
) -> BTreeMap<(PathBuf, String), Vec<char>> {
    let mut blocks = BTreeMap::new();
    for file in files {
        for task in &file.tasks {
            if let Some(block_id) = &task.block_id {
                blocks
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert_with(Vec::new)
                    .push(task.status);
            }
        }
    }
    blocks
}
