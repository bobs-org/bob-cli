use std::{fs, path::Path};

use serde::Serialize;
use serde_json::Value;

use super::{bob_env, print_json, DataviewError, OutputFormat};

use self::{
    index::TaskIndex, parse::QueryAst, result::TaskResult,
    settings::TasksSettings,
};

mod filter;
mod index;
mod js;
mod parse;
mod render;
mod result;
mod settings;
mod task;

struct Execution {
    query: QueryAst,
    result: TaskResult,
    function_groups: Value,
    paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteBlock {
    index: usize,
    line_number: usize,
    heading: Option<String>,
    query: String,
}

impl NoteBlock {
    fn label(&self, note: &Path) -> String {
        let context = self
            .heading
            .as_deref()
            .map(|heading| format!("{note}#{heading}", note = note.display()))
            .unwrap_or_else(|| note.display().to_string());
        format!(
            "{context} (block {index}, line {line})",
            index = self.index,
            line = self.line_number + 1
        )
    }

    fn display_heading(&self) -> String {
        self.heading
            .clone()
            .unwrap_or_else(|| "Tasks query".to_string())
    }
}

pub(super) fn run(
    vault: &Path,
    origin: Option<&Path>,
    query: &str,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    let settings = TasksSettings::read(vault)?;
    let now = bob_env::current_datetime();
    let index = TaskIndex::read(vault, &settings, now)?;
    let execution = execute(vault, origin, query, &settings, &index, now)?;
    emit_single(execution, &settings, format)
}

pub(super) fn run_note(
    vault: &Path,
    note: &Path,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    let note_path = vault.join(note);
    let contents = fs::read_to_string(&note_path).map_err(|error| {
        DataviewError::NativeVaultRead {
            path: note_path,
            error,
        }
    })?;
    let blocks = extract_note_blocks(&contents);
    let settings = TasksSettings::read(vault)?;
    let now = bob_env::current_datetime();
    let index = TaskIndex::read(vault, &settings, now)?;
    let mut executions = Vec::with_capacity(blocks.len());
    for block in &blocks {
        let execution =
            execute(vault, Some(note), &block.query, &settings, &index, now)
                .map_err(|error| add_block_context(error, block, note))?;
        executions.push(execution);
    }
    emit_note(note, &blocks, &executions, &settings, format)
}

fn execute(
    vault: &Path,
    origin: Option<&Path>,
    source: &str,
    settings: &TasksSettings,
    index: &TaskIndex,
    now: chrono::NaiveDateTime,
) -> Result<Execution, DataviewError> {
    let query = parse::parse(vault, origin, source, settings)?;
    let all_tasks = index.tasks.clone();
    let mut javascript =
        js::JsSandbox::new(&index.tasks, query.context.as_ref(), now)?;
    let tasks = filter::apply(
        &query.filters,
        index.tasks.clone(),
        now,
        &settings.global_filter,
        &mut javascript,
    )?;
    let result = result::build(
        &query,
        tasks,
        all_tasks,
        now,
        &settings.global_filter,
        &mut javascript,
    )?;
    let function_groups =
        javascript.function_groups(&query.grouping, &result.tasks);
    let paths = result.paths();
    Ok(Execution {
        query,
        result,
        function_groups,
        paths,
    })
}

fn emit_single(
    execution: Execution,
    settings: &TasksSettings,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    match format {
        OutputFormat::Paths => {
            if !execution.paths.is_empty() {
                println!("{}", execution.paths.join("\n"));
            }
            Ok(())
        }
        OutputFormat::Json => print_json(serde_json::json!({
            "engine": "native",
            "query_kind": "tasks",
            "format": "json",
            "query": execution.query,
            "paths": execution.paths,
            "result": result_json(&execution.result, execution.function_groups),
            "settings": settings,
            "warnings": [],
        })),
        OutputFormat::Markdown => {
            let markdown = render::markdown(
                &execution.result,
                &execution.query,
                settings.task_format,
            );
            if !markdown.is_empty() {
                println!("{markdown}");
            }
            Ok(())
        }
    }
}

fn emit_note(
    note: &Path,
    blocks: &[NoteBlock],
    executions: &[Execution],
    settings: &TasksSettings,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    match format {
        OutputFormat::Paths => {
            let output = blocks
                .iter()
                .zip(executions)
                .map(|(block, execution)| {
                    let mut lines = vec![format!("[{}]", block.label(note))];
                    lines.extend(execution.paths.iter().cloned());
                    lines.join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        OutputFormat::Json => {
            let blocks = blocks
                .iter()
                .zip(executions)
                .map(|(block, execution)| {
                    serde_json::json!({
                        "index": block.index,
                        "lineNumber": block.line_number,
                        "heading": block.heading,
                        "query": block.query,
                        "paths": execution.paths,
                        "parsedQuery": execution.query,
                        "result": result_json(
                            &execution.result,
                            execution.function_groups.clone(),
                        ),
                    })
                })
                .collect::<Vec<_>>();
            let paths = executions
                .iter()
                .flat_map(|execution| execution.paths.iter())
                .fold(Vec::<String>::new(), |mut paths, path| {
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                    paths
                });
            print_json(serde_json::json!({
                "engine": "native",
                "query_kind": "tasks_note",
                "format": "json",
                "note": note.to_string_lossy().replace('\\', "/"),
                "paths": paths,
                "blocks": blocks,
                "settings": settings,
                "warnings": [],
            }))
        }
        OutputFormat::Markdown => {
            let output = blocks
                .iter()
                .zip(executions)
                .map(|(block, execution)| {
                    let heading = format!(
                        "## {} (block {})",
                        block.display_heading(),
                        block.index
                    );
                    let markdown = render::markdown(
                        &execution.result,
                        &execution.query,
                        settings.task_format,
                    );
                    if markdown.is_empty() {
                        heading
                    } else {
                        format!("{heading}\n\n{markdown}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
    }
}

fn result_json(result: &TaskResult, function_groups: Value) -> Value {
    serde_json::json!({
        "type": "tasks",
        "count": result.count,
        "countBeforeLimit": result.count_before_limit,
        "countText": result.count_text,
        "tasks": result.tasks,
        "groups": result.groups,
        "explanation": result.explanation,
        "functionGroups": function_groups,
    })
}

fn add_block_context(
    error: DataviewError,
    block: &NoteBlock,
    note: &Path,
) -> DataviewError {
    match error {
        DataviewError::TasksQuery { message } => DataviewError::TasksQuery {
            message: format!("{}: {message}", block.label(note)),
        },
        other => other,
    }
}

fn extract_note_blocks(contents: &str) -> Vec<NoteBlock> {
    struct Fence {
        marker: char,
        length: usize,
        tasks: bool,
        line_number: usize,
        heading: Option<String>,
        query: Vec<String>,
    }

    let mut heading = None;
    let mut fence: Option<Fence> = None;
    let mut blocks = Vec::new();

    for (line_number, line) in contents.lines().enumerate() {
        if let Some(open) = fence.as_mut() {
            if is_closing_fence(line, open.marker, open.length) {
                let open = fence.take().expect("open fence exists");
                if open.tasks {
                    blocks.push(NoteBlock {
                        index: blocks.len() + 1,
                        line_number: open.line_number,
                        heading: open.heading,
                        query: open.query.join("\n"),
                    });
                }
            } else if open.tasks {
                open.query.push(line.to_string());
            }
            continue;
        }

        if let Some((marker, length, info)) = opening_fence(line) {
            fence = Some(Fence {
                marker,
                length,
                tasks: info.split_whitespace().next().is_some_and(|language| {
                    language.eq_ignore_ascii_case("tasks")
                }),
                line_number,
                heading: heading.clone(),
                query: Vec::new(),
            });
            continue;
        }

        if let Some(value) = atx_heading(line) {
            heading = Some(value);
        }
    }

    if let Some(open) = fence
        && open.tasks
    {
        blocks.push(NoteBlock {
            index: blocks.len() + 1,
            line_number: open.line_number,
            heading: open.heading,
            query: open.query.join("\n"),
        });
    }
    blocks
}

fn opening_fence(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let line = &line[indent..];
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = line.chars().take_while(|value| *value == marker).count();
    (length >= 3).then(|| (marker, length, line[length..].trim()))
}

fn is_closing_fence(line: &str, marker: char, minimum: usize) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let line = &line[indent..];
    let length = line.chars().take_while(|value| *value == marker).count();
    length >= minimum && line[length..].trim().is_empty()
}

fn atx_heading(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let line = &line[indent..];
    let hashes = line.chars().take_while(|value| *value == '#').count();
    if !(1..=6).contains(&hashes)
        || !line[hashes..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        return None;
    }
    let value = line[hashes..]
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tasks_fences_with_heading_context() {
        let blocks = extract_note_blocks(concat!(
            "# Dashboard\n",
            "```rust\n# not a heading\n```\n",
            "## WIP\n",
            "```tasks\nstatus.type is IN_PROGRESS\n```\n",
            "### Ready ###\n",
            "  ~~~~Tasks extra-info\nstatus.type is TODO\n~~~~\n",
        ));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].index, 1);
        assert_eq!(blocks[0].line_number, 5);
        assert_eq!(blocks[0].heading.as_deref(), Some("WIP"));
        assert_eq!(blocks[0].query, "status.type is IN_PROGRESS");
        assert_eq!(blocks[1].heading.as_deref(), Some("Ready"));
        assert_eq!(blocks[1].query, "status.type is TODO");
    }
}
