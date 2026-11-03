//! Shared parsing and resolution for task locators.

use std::{collections::BTreeMap, path::PathBuf};

use super::{
    note_tasks::{self, BlockIdLookup, NoteTask, NoteTaskSettings},
    vault_links::{LinkResolution, NoteIndex},
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
pub(crate) struct TaskLocator {
    pub(crate) note: String,
    pub(crate) block_id: String,
}

#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
impl TaskLocator {
    /// Parse `note:block-id` (using the last colon) or `[[note#^block-id]]`.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(inner) =
            value.strip_prefix("[[").and_then(|s| s.strip_suffix("]]"))
        {
            let (note, fragment) = inner.split_once("#^")?;
            return Self::new(note, fragment);
        }
        let (note, block_id) = value.rsplit_once(':')?;
        Self::new(note, block_id.strip_prefix('^').unwrap_or(block_id))
    }

    fn new(note: &str, block_id: &str) -> Option<Self> {
        let note = note.trim();
        let block_id = block_id.trim();
        if note.is_empty()
            || block_id.is_empty()
            || block_id.chars().any(char::is_whitespace)
            || block_id.contains(['#', '^', '[', ']'])
        {
            return None;
        }
        Some(Self {
            note: note.to_string(),
            block_id: block_id.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
pub(crate) struct LocatedTask {
    pub(crate) path: PathBuf,
    /// One-based physical line number.
    pub(crate) line: usize,
    pub(crate) task: NoteTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
pub(crate) struct TaskLocatorError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
impl TaskLocatorError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Resolve a locator against a vault snapshot, preserving the matched
/// `NoteTask` so writers can use its parsed fields without rescanning.
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge and split phases.
pub(crate) fn resolve_task(
    snapshot: &BTreeMap<PathBuf, String>,
    locator: &str,
    settings: &NoteTaskSettings,
) -> Result<LocatedTask, TaskLocatorError> {
    let locator = TaskLocator::parse(locator).ok_or_else(|| {
        TaskLocatorError::new(
            "task_not_found",
            format!("`{locator}` is not a task locator; use note:block-id or [[note#^block-id]]"),
        )
    })?;
    let note_index = NoteIndex::from_paths(snapshot.keys().cloned());
    let path = match note_index.resolve_detailed(None, &locator.note) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return Err(TaskLocatorError::new(
                "ambiguous_note",
                format!("`{}` matches more than one note", locator.note),
            ));
        }
        LinkResolution::Missing => {
            return Err(TaskLocatorError::new(
                "task_not_found",
                format!("no note matches `{}`", locator.note),
            ));
        }
    };
    let Some(contents) = snapshot.get(&path) else {
        return Err(TaskLocatorError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", locator.block_id, path.display()),
        ));
    };
    let scan = note_tasks::scan(contents, settings);
    match scan.by_block_id(&locator.block_id) {
        BlockIdLookup::Found(task) => Ok(LocatedTask {
            path,
            line: task.line_index + 1,
            task: task.clone(),
        }),
        BlockIdLookup::NotATask {
            line_index,
            excerpt,
        } => Err(TaskLocatorError::new(
            "not_a_task",
            format!(
                "^{}` in `{}` is not a task (line {}: {})",
                locator.block_id,
                path.display(),
                line_index + 1,
                excerpt
            ),
        )),
        BlockIdLookup::Duplicate(count) => Err(TaskLocatorError::new(
            "task_not_found",
            format!(
                "^{}` in `{}` is ambiguous ({count} block IDs); make it unique",
                locator.block_id,
                path.display()
            ),
        )),
        BlockIdLookup::Missing => Err(TaskLocatorError::new(
            "task_not_found",
            format!("no task `^{}` in `{}`", locator.block_id, path.display()),
        )),
    }
}

#[cfg(test)]
mod tests;
