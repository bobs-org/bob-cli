//! Obsidian Tasks settings read, parse, and validation.
use super::*;

pub(crate) fn read_tasks_settings(vault: &Path) -> TasksSettings {
    let path = vault.join(TASKS_SETTINGS);
    match fs::read_to_string(&path) {
        Ok(contents) => parse_tasks_settings(&path, Some(&contents)),
        Err(_) => parse_tasks_settings(&path, None),
    }
}

pub(super) fn parse_tasks_settings(
    path: &Path,
    contents: Option<&str>,
) -> TasksSettings {
    let mut settings = TasksSettings {
        global_filter: DEFAULT_GLOBAL_FILTER.to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: Vec::new(),
        status_settings_error: None,
    };
    let Some(contents) = contents else {
        settings.status_settings_error =
            Some(format!("Tasks settings are missing at {}", path.display()));
        return settings;
    };
    let Ok(value) = serde_json::from_str::<Value>(contents) else {
        settings.status_settings_error = Some(format!(
            "Tasks settings are not valid JSON at {}",
            path.display()
        ));
        return settings;
    };
    settings.global_filter = value
        .get("globalFilter")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_GLOBAL_FILTER)
        .to_string();
    let mut configured_symbols = BTreeSet::new();
    for collection in ["coreStatuses", "customStatuses"] {
        let statuses = value
            .get("statusSettings")
            .and_then(|settings| settings.get(collection))
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for status in statuses {
            let symbol = status
                .get("symbol")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status_type = TaskStatusType::from_settings(
                status.get("type").and_then(Value::as_str).unwrap_or("TODO"),
            );
            let definition = TaskStatusDefinition {
                symbol: symbol.to_string(),
                name: status
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                next_status_symbol: status
                    .get("nextStatusSymbol")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                available_as_command: status
                    .get("availableAsCommand")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                status_type,
            };
            settings.status_definitions.push(definition);

            let mut chars = symbol.chars();
            if let Some(symbol) = chars.next()
                && chars.next().is_none()
            {
                if configured_symbols.insert(symbol) {
                    settings.status_types.insert(symbol, status_type);
                }
                if status_type == TaskStatusType::Done {
                    settings.done_statuses.insert(symbol);
                }
            }
        }
    }
    settings
}

pub(crate) fn validate_blocked_status(
    settings: &TasksSettings,
) -> Result<(), SyncError> {
    if let Some(error) = &settings.status_settings_error {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: {error}; configure one custom status named Blocked with symbol '?', type ON_HOLD, next status ' ', and availableAsCommand true"
        )));
    }
    let candidates = settings
        .status_definitions
        .iter()
        .filter(|definition| {
            definition.symbol == "?"
                || definition.name.eq_ignore_ascii_case("Blocked")
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(SyncError::new(
            "cannot reconcile Blocked [?] tasks: Tasks settings have no Blocked status; configure one custom status named Blocked with symbol '?', type ON_HOLD, next status ' ', and availableAsCommand true",
        ));
    }
    if candidates.len() > 1 {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: Tasks settings contain {} definitions using symbol '?' or name Blocked; keep exactly one compatible definition",
            candidates.len()
        )));
    }
    let definition = candidates[0];
    if definition.symbol != "?"
        || definition.name != "Blocked"
        || definition.status_type != TaskStatusType::OnHold
        || definition.next_status_symbol != " "
        || !definition.available_as_command
    {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: the Tasks status is incompatible (symbol={:?}, name={:?}, type={:?}, next={:?}, availableAsCommand={}); expected symbol '?', name Blocked, type ON_HOLD, next status ' ', and availableAsCommand true",
            definition.symbol,
            definition.name,
            definition.status_type,
            definition.next_status_symbol,
            definition.available_as_command
        )));
    }
    Ok(())
}
