use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{print_json, DataviewError, OutputFormat};

use self::settings::TasksSettings;

mod settings;

#[derive(Debug, Serialize)]
struct IndexedTask {
    path: String,
    status: char,
    text: String,
}

pub(super) fn run(
    vault: &Path,
    query: &str,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    let settings = TasksSettings::read(vault)?;
    validate_filterless_query(query, &settings.global_query)?;
    let tasks = read_tasks(vault, &settings.global_filter)?;
    let paths = tasks
        .iter()
        .map(|task| task.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Paths => {
            if !paths.is_empty() {
                println!("{}", paths.join("\n"));
            }
            Ok(())
        }
        OutputFormat::Json => print_json(serde_json::json!({
            "engine": "native",
            "query_kind": "tasks",
            "format": "json",
            "paths": paths,
            "result": {
                "type": "tasks",
                "count": tasks.len(),
                "tasks": tasks,
            },
            "settings": settings,
            "warnings": [],
        })),
        OutputFormat::Markdown => Err(DataviewError::TasksQuery {
            message: "markdown output is not available for Tasks queries yet"
                .to_string(),
        }),
    }
}

fn validate_filterless_query(
    query: &str,
    global_query: &str,
) -> Result<(), DataviewError> {
    if let Some(instruction) = first_instruction(global_query) {
        return Err(DataviewError::TasksQuery {
            message: format!(
                "the configured global query is not supported by the current \
                 filterless Tasks slice: {instruction}"
            ),
        });
    }
    if let Some(instruction) = first_instruction(query) {
        return Err(DataviewError::TasksQuery {
            message: format!(
                "only an empty or comment-only Tasks query is supported yet; \
                 unsupported instruction: {instruction}"
            ),
        });
    }
    Ok(())
}

fn first_instruction(query: &str) -> Option<&str> {
    query
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
}

fn read_tasks(
    vault: &Path,
    global_filter: &str,
) -> Result<Vec<IndexedTask>, DataviewError> {
    let mut markdown_paths = Vec::new();
    collect_markdown_paths(vault, &mut markdown_paths)?;
    markdown_paths.sort();

    let mut tasks = Vec::new();
    for path in markdown_paths {
        let contents = fs::read_to_string(&path).map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        let relative_path = path.strip_prefix(vault).map_err(|error| {
            DataviewError::TasksQuery {
                message: format!(
                    "failed to make {} vault-relative: {error}",
                    path.display()
                ),
            }
        })?;
        let relative_path = relative_path.to_string_lossy().replace('\\', "/");

        let mut fence = None;
        for line in contents.lines() {
            if update_fence(line, &mut fence) {
                continue;
            }
            if fence.is_some() {
                continue;
            }
            let Some((status, text)) = parse_task_line(line) else {
                continue;
            };
            if !global_filter.is_empty() && !text.contains(global_filter) {
                continue;
            }
            tasks.push(IndexedTask {
                path: relative_path.clone(),
                status,
                text: text.to_string(),
            });
        }
    }
    Ok(tasks)
}

fn collect_markdown_paths(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), DataviewError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        }
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        if file_type.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                collect_markdown_paths(&path, paths)?;
            }
        } else if file_type.is_file() && is_markdown(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn parse_task_line(line: &str) -> Option<(char, &str)> {
    let line = line.trim_start_matches([' ', '\t']);
    let after_marker = if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        rest
    } else {
        let digit_count = line.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            return None;
        }
        let rest = &line[digit_count..];
        rest.strip_prefix(". ")
            .or_else(|| rest.strip_prefix(") "))?
    };

    let after_open = after_marker.strip_prefix('[')?;
    let mut chars = after_open.chars();
    let status = chars.next()?;
    let after_status = chars.as_str().strip_prefix(']')?;
    if !after_status.is_empty()
        && !after_status.starts_with(char::is_whitespace)
    {
        return None;
    }
    Some((status, after_status.trim_start()))
}

fn update_fence(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next();
    let Some(marker @ ('`' | '~')) = marker else {
        return false;
    };
    let count = trimmed.chars().take_while(|value| *value == marker).count();
    if count < 3 {
        return false;
    }

    match *fence {
        Some((open_marker, open_count))
            if marker == open_marker && count >= open_count =>
        {
            *fence = None;
        }
        None => *fence = Some((marker, count)),
        Some(_) => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_line_parser_accepts_supported_markers() {
        for line in [
            "- [ ] task",
            "  * [/] task",
            "\t+ [*] task",
            "1. [x] task",
            "22) [-] task",
        ] {
            assert!(parse_task_line(line).is_some(), "{line}");
        }
    }

    #[test]
    fn task_line_parser_rejects_non_tasks() {
        for line in ["- plain", "text - [ ] task", "- [] task", "- [ ]task"] {
            assert!(parse_task_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn comments_are_filterless_query_input() {
        assert_eq!(first_instruction("\n# comment\n  # another\n"), None);
        assert_eq!(
            first_instruction("# comment\nstatus.type is TODO"),
            Some("status.type is TODO")
        );
    }
}
