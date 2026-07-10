use std::path::Path;

use super::{bob_env, print_json, DataviewError, OutputFormat};

use self::{index::TaskIndex, settings::TasksSettings};

mod filter;
mod index;
mod js;
mod parse;
mod render;
mod result;
mod settings;
mod task;

pub(super) fn run(
    vault: &Path,
    origin: Option<&Path>,
    query: &str,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    let settings = TasksSettings::read(vault)?;
    let query = parse::parse(vault, origin, query, &settings)?;
    let now = bob_env::current_datetime();
    let index = TaskIndex::read(vault, &settings, now)?;
    let all_tasks = index.tasks.clone();
    let mut javascript =
        js::JsSandbox::new(&index.tasks, query.context.as_ref(), now)?;
    let tasks = filter::apply(
        &query.filters,
        index.tasks,
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
            "query": query,
            "paths": paths,
            "result": {
                "type": "tasks",
                "count": result.count,
                "countBeforeLimit": result.count_before_limit,
                "countText": result.count_text,
                "tasks": result.tasks,
                "groups": result.groups,
                "explanation": result.explanation,
                "functionGroups": function_groups,
            },
            "settings": settings,
            "warnings": [],
        })),
        OutputFormat::Markdown => {
            let markdown =
                render::markdown(&result, &query, settings.task_format);
            if !markdown.is_empty() {
                println!("{markdown}");
            }
            Ok(())
        }
    }
}
