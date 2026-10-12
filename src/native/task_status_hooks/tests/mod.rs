//! Shared fixtures for task-status hook tests.
pub(super) use super::*;

fn reference(target: &str, block_id: &str) -> RawReference {
    RawReference {
        target: target.to_string(),
        block_id: block_id.to_string(),
    }
}

fn identity(block_id: &str) -> (PathBuf, String) {
    (PathBuf::from("tasks.md"), block_id.to_string())
}

fn test_settings() -> TasksSettings {
    TasksSettings {
        global_filter: "#task".to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::OnHold),
            ('?', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: vec![TaskStatusDefinition {
            symbol: "?".to_string(),
            name: "Blocked".to_string(),
            next_status_symbol: " ".to_string(),
            available_as_command: true,
            status_type: TaskStatusType::OnHold,
        }],
        status_settings_error: None,
    }
}

fn resolved(
    values: &[(&str, &str, Vec<char>)],
) -> BTreeMap<RawReference, ResolvedReference> {
    values
        .iter()
        .map(|(target, block_id, statuses)| {
            (
                reference(target, block_id),
                ResolvedReference {
                    path: PathBuf::from(format!("{target}.md")),
                    statuses: statuses.clone(),
                },
            )
        })
        .collect()
}

fn resolved_paths(
    values: &[(&str, &str, &str, Vec<char>)],
) -> BTreeMap<RawReference, ResolvedReference> {
    values
        .iter()
        .map(|(target, block_id, path, statuses)| {
            (
                reference(target, block_id),
                ResolvedReference {
                    path: PathBuf::from(path),
                    statuses: statuses.clone(),
                },
            )
        })
        .collect()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

mod structure;
mod sync;
