use std::{collections::BTreeMap, fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::super::DataviewError;

const SETTINGS_RELATIVE_PATH: &str =
    ".obsidian/plugins/obsidian-tasks-plugin/data.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct TasksSettings {
    pub(super) global_filter: String,
    pub(super) global_query: String,
    pub(super) remove_global_filter: bool,
    pub(super) task_format: TaskFormat,
    pub(super) status_settings: StatusSettings,
    #[serde(default = "default_presets")]
    pub(super) presets: BTreeMap<String, String>,
}

impl TasksSettings {
    pub(super) fn read(vault: &Path) -> Result<Self, DataviewError> {
        let path = vault.join(SETTINGS_RELATIVE_PATH);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(DataviewError::TasksSettingsRead { path, error });
            }
        };

        serde_json::from_str(&contents)
            .map_err(|error| DataviewError::TasksSettingsParse { path, error })
    }
}

impl Default for TasksSettings {
    fn default() -> Self {
        Self {
            global_filter: String::new(),
            global_query: String::new(),
            remove_global_filter: false,
            task_format: TaskFormat::Emoji,
            status_settings: StatusSettings::default(),
            presets: default_presets(),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize,
)]
pub(super) enum TaskFormat {
    #[serde(rename = "dataview")]
    Dataview,
    #[default]
    #[serde(rename = "tasksPluginEmoji", alias = "emoji")]
    Emoji,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct StatusSettings {
    #[serde(default = "default_core_statuses")]
    pub(super) core_statuses: Vec<TaskStatus>,
    #[serde(default = "default_custom_statuses")]
    pub(super) custom_statuses: Vec<TaskStatus>,
}

impl Default for StatusSettings {
    fn default() -> Self {
        Self {
            core_statuses: default_core_statuses(),
            custom_statuses: default_custom_statuses(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct TaskStatus {
    pub(super) symbol: String,
    pub(super) name: String,
    pub(super) next_status_symbol: String,
    pub(super) available_as_command: bool,
    #[serde(rename = "type")]
    pub(super) status_type: String,
}

fn default_core_statuses() -> Vec<TaskStatus> {
    vec![
        TaskStatus {
            symbol: " ".to_string(),
            name: "Todo".to_string(),
            next_status_symbol: "x".to_string(),
            available_as_command: true,
            status_type: "TODO".to_string(),
        },
        TaskStatus {
            symbol: "x".to_string(),
            name: "Done".to_string(),
            next_status_symbol: " ".to_string(),
            available_as_command: true,
            status_type: "DONE".to_string(),
        },
    ]
}

fn default_custom_statuses() -> Vec<TaskStatus> {
    vec![
        TaskStatus {
            symbol: "/".to_string(),
            name: "In Progress".to_string(),
            next_status_symbol: "x".to_string(),
            available_as_command: true,
            status_type: "IN_PROGRESS".to_string(),
        },
        TaskStatus {
            symbol: "-".to_string(),
            name: "Cancelled".to_string(),
            next_status_symbol: " ".to_string(),
            available_as_command: true,
            status_type: "CANCELLED".to_string(),
        },
    ]
}

fn default_presets() -> BTreeMap<String, String> {
    [
        (
            "this_file",
            "path includes {{query.file.path}}",
        ),
        (
            "this_folder",
            "folder includes {{query.file.folder}}",
        ),
        (
            "this_folder_only",
            "filter by function task.file.folder === query.file.folder",
        ),
        ("this_root", "root includes {{query.file.root}}"),
        (
            "hide_date_fields",
            "# Hide any values for all date fields\nhide due date\nhide scheduled date\nhide start date\nhide created date\nhide done date\nhide cancelled date",
        ),
        (
            "hide_non_date_fields",
            "# Hide all the non-date fields, but not tags\nhide id\nhide depends on\nhide recurrence rule\nhide on completion\nhide priority",
        ),
        (
            "hide_query_elements",
            "# Hide toolbar, postpone, edit and backlinks\nhide toolbar\nhide postpone button\nhide edit button\nhide backlinks",
        ),
        (
            "hide_everything",
            "# Hide everything except description and any tags\npreset hide_date_fields\npreset hide_non_date_fields\npreset hide_query_elements",
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}
