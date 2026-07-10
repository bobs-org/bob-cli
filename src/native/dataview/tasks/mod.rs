use std::{collections::BTreeSet, path::Path};

use super::{bob_env, print_json, DataviewError, OutputFormat};

use self::{index::TaskIndex, settings::TasksSettings};

mod index;
mod parse;
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
    let index = TaskIndex::read(vault, &settings, bob_env::current_datetime())?;
    let paths = index
        .tasks
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
            "query": query,
            "paths": paths,
            "result": {
                "type": "tasks",
                "count": index.tasks.len(),
                "tasks": index.tasks,
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
