use std::{collections::BTreeSet, path::Path};

use super::{bob_env, print_json, DataviewError, OutputFormat};

use self::{index::TaskIndex, settings::TasksSettings};

mod index;
mod settings;
mod task;

pub(super) fn run(
    vault: &Path,
    query: &str,
    format: OutputFormat,
) -> Result<(), DataviewError> {
    let settings = TasksSettings::read(vault)?;
    validate_filterless_query(query, &settings.global_query)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_filterless_query_input() {
        assert_eq!(first_instruction("\n# comment\n  # another\n"), None);
        assert_eq!(
            first_instruction("# comment\nstatus.type is TODO"),
            Some("status.type is TODO")
        );
    }
}
