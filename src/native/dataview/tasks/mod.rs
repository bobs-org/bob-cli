use std::{fs, path::Path};

use chrono::NaiveDate;
use serde::Serialize;
use serde_json::Value;

use super::{bob_env, print_json, DataviewError, OutputFormat};
use crate::native::note_tasks::clean_description;

use self::{
    index::TaskIndex, parse::QueryAst, result::TaskResult,
    settings::TasksSettings, task::TaskDate,
};

mod filter;
mod index;
mod js;
mod parse;
mod render;
mod result;
mod settings;
mod task;

pub(crate) use settings::TaskFormat;
pub(crate) use task::tasks_fingerprint;
#[cfg(test)]
pub(crate) use task::{parse_details, TaskDetails};

struct Execution {
    query: QueryAst,
    result: TaskResult,
    function_groups: Value,
    paths: Vec<String>,
}

struct NoteExecution {
    execution: Option<Execution>,
    error: Option<String>,
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

/// The `docs/plan.md` NEXT lane query: every Next task visible
/// today. It mirrors the old NOW query's defaults, replacing only the
/// tag test with the status test, so the dash and `bob plan` agree.
///
/// `status.symbol is` is bob-cli native-internal syntax (see
/// [`parse::ParseDialect`]): it parses only in the native dialect for these
/// internal lane constants and is rejected in user-authored Obsidian Tasks
/// blocks, where the dashboard NEXT block uses
/// `filter by function task.status.symbol === "*"`.
pub(crate) const NEXT_QUERY: &str = "not done\nstatus.symbol is *\nis not blocked\ntags do not include #hide\nfolder does not include _templates\npath does not include _conflicts\n(no scheduled date) OR (scheduled on or before today)";

/// The `docs/plan.md` PENDING lane query: every In Progress task
/// visible today. Same defaults as [`NEXT_QUERY`].
///
/// Like [`NEXT_QUERY`], `status.symbol is` here is native-internal syntax
/// (see [`parse::ParseDialect`]), not valid Obsidian Tasks syntax; the
/// dashboard PENDING block uses `status.type is IN_PROGRESS`.
pub(crate) const PENDING_QUERY: &str = "not done\nstatus.symbol is /\nis not blocked\ntags do not include #hide\nfolder does not include _templates\npath does not include _conflicts\n(no scheduled date) OR (scheduled on or before today)";

/// The `docs/freshness.md` READY lane query: every Ready task
/// visible today. It mirrors [`NEXT_QUERY`]'s defaults, replacing only
/// the status test with the status-type TODO test, so the review queue
/// and `bob plan` agree on lane visibility.
pub(crate) const READY_QUERY: &str = "not done\nstatus.type is TODO\nis not blocked\ntags do not include #hide\nfolder does not include _templates\npath does not include _conflicts\n(no scheduled date) OR (scheduled on or before today)";

/// A looser open-task query for the freshness seed universe and the
/// `refreshed_today` base: every open task, whatever its lane.
pub(crate) const OPEN_QUERY: &str = "not done";

/// One task row for the freshness review queue and seed, read through
/// the native Tasks engine so custom statuses, blocked derivation,
/// and the vault's Tasks settings are honored exactly as in
/// `bob plan`.
#[derive(Debug, Clone)]
pub(crate) struct RichTask {
    /// Vault-relative path with forward slashes.
    pub(crate) path: String,
    /// 1-based line number, as in JSON and docs.
    pub(crate) line: u32,
    pub(crate) status_symbol: String,
    /// Tasks status type (`TODO`, `DONE`, `IN_PROGRESS`, …).
    pub(crate) status_type: String,
    pub(crate) original_markdown: String,
    /// Clean description: inline fields, block ID, and the global
    /// filter stripped, as `bob freshness list -f json` reports it.
    pub(crate) text: String,
    pub(crate) created: Option<NaiveDate>,
    pub(crate) scheduled: Option<NaiveDate>,
    pub(crate) is_recurring: bool,
    pub(crate) is_blocked: bool,
    pub(crate) tags: Vec<String>,
    pub(crate) block_id: Option<String>,
}

/// The vault's Tasks format, so `bob freshness` can refuse to run on
/// a non-Dataview vault (see `docs/freshness.md` rule 1).
pub(crate) fn read_task_format(
    vault: &Path,
) -> Result<TaskFormat, DataviewError> {
    Ok(TasksSettings::read(vault)?.task_format)
}

/// Every task in the vault, unfiltered: the base for
/// `refreshed_today` (tasks of any status) and freshness warnings.
pub(crate) fn scan_all_rich_tasks(
    vault: &Path,
    now: chrono::NaiveDateTime,
) -> Result<Vec<RichTask>, DataviewError> {
    let settings = TasksSettings::read(vault)?;
    let index = TaskIndex::read(vault, &settings, now)?;
    Ok(index
        .tasks
        .iter()
        .map(|task| rich_task(task, &settings))
        .collect())
}

/// The descriptions of the tasks matching `query` through the native
/// Tasks engine, so plan-budget lane counts honor the vault's Tasks
/// settings. Callers apply their own whole-token tag predicates (the
/// engine's `tags include` also matches subtags like `#hide/x`).
pub(crate) fn query_matching_descriptions(
    vault: &Path,
    query: &str,
    now: chrono::NaiveDateTime,
) -> Result<Vec<String>, DataviewError> {
    let settings = TasksSettings::read(vault)?;
    let index = TaskIndex::read(vault, &settings, now)?;
    let parsed = parse::parse(
        vault,
        None,
        query,
        &settings,
        parse::ParseDialect::Native,
    )?;
    let mut javascript = maybe_sandbox(&parsed, &index.tasks, now)?;
    let execution =
        execute_query(parsed, &settings, &index, now, javascript.as_mut())?;
    Ok(execution
        .result
        .tasks
        .iter()
        .map(|task| task.description.clone())
        .collect())
}

/// The rich rows matching `query` through the native Tasks engine.
/// See [`query_matching_descriptions`] for the engine contract.
pub(crate) fn query_rich_tasks(
    vault: &Path,
    query: &str,
    now: chrono::NaiveDateTime,
) -> Result<Vec<RichTask>, DataviewError> {
    let settings = TasksSettings::read(vault)?;
    let index = TaskIndex::read(vault, &settings, now)?;
    let parsed = parse::parse(
        vault,
        None,
        query,
        &settings,
        parse::ParseDialect::Native,
    )?;
    let mut javascript = maybe_sandbox(&parsed, &index.tasks, now)?;
    let execution =
        execute_query(parsed, &settings, &index, now, javascript.as_mut())?;
    Ok(execution
        .result
        .tasks
        .iter()
        .map(|task| rich_task(task, &settings))
        .collect())
}

/// Build the JavaScript sandbox only when `query` needs it: a query
/// without `by function` never pays Moment parsing or whole-vault
/// hydration and never depends on the sandbox deadline (bob-cli-33).
fn maybe_sandbox(
    query: &parse::QueryAst,
    tasks: &[task::Task],
    now: chrono::NaiveDateTime,
) -> Result<Option<js::JsSandbox>, DataviewError> {
    if query.uses_javascript() {
        js::JsSandbox::new(tasks, query.context.as_ref(), now).map(Some)
    } else {
        Ok(None)
    }
}

fn rich_task(task: &task::Task, settings: &TasksSettings) -> RichTask {
    RichTask {
        path: task.path.clone(),
        line: u32::try_from(task.line_number + 1).unwrap_or(u32::MAX),
        status_symbol: task.status.symbol.clone(),
        status_type: task.status.status_type.as_str().to_string(),
        original_markdown: task.original_markdown.clone(),
        text: clean_description(&task.text, &settings.global_filter, None),
        created: task.created.as_ref().and_then(TaskDate::valid_date),
        scheduled: task.scheduled.as_ref().and_then(TaskDate::valid_date),
        is_recurring: task.is_recurring,
        is_blocked: task.is_blocked,
        tags: task.tags.clone(),
        block_id: task.block_id.clone(),
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
    let query = parse::parse(
        vault,
        origin,
        query,
        &settings,
        parse::ParseDialect::Upstream,
    )?;
    let mut javascript = maybe_sandbox(&query, &index.tasks, now)?;
    let execution =
        execute_query(query, &settings, &index, now, javascript.as_mut())?;
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
    let parsed = blocks
        .iter()
        .map(|block| {
            parse::parse(
                vault,
                Some(note),
                &block.query,
                &settings,
                parse::ParseDialect::Upstream,
            )
        })
        .collect::<Vec<_>>();
    let first_javascript_query = parsed
        .iter()
        .filter_map(|query| query.as_ref().ok())
        .find(|query| query.uses_javascript());
    let mut javascript = first_javascript_query
        .map(|query| {
            js::JsSandbox::new(&index.tasks, query.context.as_ref(), now)
        })
        .transpose()?;
    let mut executions = Vec::with_capacity(blocks.len());
    for (block, query) in blocks.iter().zip(parsed) {
        let result = query.and_then(|query| {
            execute_query(query, &settings, &index, now, javascript.as_mut())
        });
        match result {
            Ok(execution) => executions.push(NoteExecution {
                execution: Some(execution),
                error: None,
            }),
            Err(error) => executions.push(NoteExecution {
                execution: None,
                error: Some(error_message(add_block_context(
                    error, block, note,
                ))),
            }),
        }
    }
    let failed = executions
        .iter()
        .filter(|result| result.error.is_some())
        .count();
    emit_note(note, &blocks, &executions, &settings, format)?;
    if failed == 0 {
        Ok(())
    } else {
        Err(DataviewError::TasksQuery {
            message: format!("{failed} Tasks block(s) failed; errors are included in the block output"),
        })
    }
}

fn execute_query(
    query: QueryAst,
    settings: &TasksSettings,
    index: &TaskIndex,
    now: chrono::NaiveDateTime,
    mut javascript: Option<&mut js::JsSandbox>,
) -> Result<Execution, DataviewError> {
    let all_tasks = index.tasks.clone();
    let tasks = filter::apply(
        &query.filters,
        index.tasks.clone(),
        now,
        &settings.global_filter,
        javascript.as_deref_mut(),
    )?;
    let result = result::build(
        &query,
        tasks,
        all_tasks,
        now,
        &settings.global_filter,
        javascript.as_deref_mut(),
    )?;
    let function_groups = javascript
        .as_mut()
        .map(|sandbox| sandbox.function_groups(&query.grouping, &result.tasks))
        .unwrap_or_else(|| Value::Array(Vec::new()));
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
                &settings.global_filter,
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
    executions: &[NoteExecution],
    settings: &TasksSettings,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    match format {
        OutputFormat::Paths => {
            let output = blocks
                .iter()
                .zip(executions)
                .map(|(block, outcome)| {
                    let mut lines = vec![format!("[{}]", block.label(note))];
                    if let Some(execution) = &outcome.execution {
                        lines.extend(execution.paths.iter().cloned());
                    }
                    if let Some(error) = &outcome.error {
                        lines.push(format!("Error: {error}"));
                    }
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
                .map(|(block, outcome)| {
                    if let Some(execution) = &outcome.execution {
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
                            "error": null,
                        })
                    } else {
                        serde_json::json!({
                            "index": block.index,
                            "lineNumber": block.line_number,
                            "heading": block.heading,
                            "query": block.query,
                            "paths": [],
                            "parsedQuery": null,
                            "result": null,
                            "error": outcome.error,
                        })
                    }
                })
                .collect::<Vec<_>>();
            let paths = executions
                .iter()
                .filter_map(|outcome| outcome.execution.as_ref())
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
                .map(|(block, outcome)| {
                    let heading = format!(
                        "## {} (block {})",
                        block.display_heading(),
                        block.index
                    );
                    let markdown = if let Some(execution) = &outcome.execution {
                        render::markdown(
                            &execution.result,
                            &execution.query,
                            settings.task_format,
                            &settings.global_filter,
                        )
                    } else {
                        format!(
                            "> [!error] Tasks query failed\n> {}",
                            outcome
                                .error
                                .as_deref()
                                .unwrap_or("unknown error")
                                .replace('\n', "\n> ")
                        )
                    };
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

fn error_message(error: DataviewError) -> String {
    match error {
        DataviewError::TasksQuery { message } => message,
        other => format!("{other:?}"),
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
                open.query.push(strip_blockquote_prefixes(line).to_string());
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
    let line = strip_blockquote_prefixes(line);
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
    let line = strip_blockquote_prefixes(line);
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let line = &line[indent..];
    let length = line.chars().take_while(|value| *value == marker).count();
    length >= minimum && line[length..].trim().is_empty()
}

fn atx_heading(line: &str) -> Option<String> {
    let line = strip_blockquote_prefixes(line);
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
    let rest = line[hashes..].trim();
    let closing_start = rest.trim_end_matches('#').len();
    let value = if closing_start < rest.len()
        && rest[..closing_start].ends_with(char::is_whitespace)
    {
        rest[..closing_start].trim().to_string()
    } else {
        rest.to_string()
    };
    (!value.is_empty()).then_some(value)
}

fn strip_blockquote_prefixes(mut line: &str) -> &str {
    loop {
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
        if spaces > 3 || line.as_bytes().get(spaces) != Some(&b'>') {
            return line;
        }
        line = &line[spaces + 1..];
        if let Some(rest) = line.strip_prefix(' ') {
            line = rest;
        }
    }
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

    #[test]
    fn non_function_queries_skip_the_sandbox_on_a_real_vault() {
        let vault = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tasks_parity/vault");
        let now = bob_env::current_datetime();
        let descriptions = query_matching_descriptions(&vault, "not done", now)
            .expect("a query without by function skips the sandbox");
        assert!(
            !descriptions.is_empty(),
            "the parity fixture vault must yield open tasks"
        );
        let rich = query_rich_tasks(&vault, "not done", now)
            .expect("a rich query without by function skips the sandbox");
        assert_eq!(rich.len(), descriptions.len());
    }

    #[test]
    fn extracts_tasks_fences_from_nested_blockquotes_and_callouts() {
        let blocks = extract_note_blocks(concat!(
            "> [!todo]\n",
            "> ```tasks\n",
            "> not done\n",
            "> ```\n",
            ">> ~~~tasks\n",
            ">> done\n",
            ">> ~~~\n",
        ));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].query, "not done");
        assert_eq!(blocks[1].query, "done");
    }
}
