//! Unit tests for the shared task-completion engine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::capture_pomodoro_close::CloseVault;
use super::super::task_status_hooks::{TaskStatusType, TasksSettings};
use super::super::vault_links::LinkResolution;
use super::*;

mod recovery_tests;
mod retirement_tests;
mod successor_tests;
mod tree_tests;

pub(super) struct MemoryVault {
    root: PathBuf,
    _directory: TempDir,
    files: BTreeMap<PathBuf, String>,
}

impl MemoryVault {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary vault");
        Self {
            root: directory.path().to_path_buf(),
            _directory: directory,
            files: BTreeMap::new(),
        }
    }

    pub(super) fn absolute(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub(super) fn insert(&mut self, relative: &str, contents: &str) {
        self.files
            .insert(self.absolute(relative), contents.to_string());
    }
}

impl CloseVault for MemoryVault {
    fn bob_dir(&self) -> &Path {
        &self.root
    }

    fn resolve_target(&self, from_path: &Path, target: &str) -> LinkResolution {
        if target.is_empty() {
            return LinkResolution::Found(from_path.to_path_buf());
        }
        let direct = self.root.join(format!("{target}.md"));
        if self.files.contains_key(&direct) {
            return LinkResolution::Found(direct);
        }
        if target.contains('/') || target.contains('\\') {
            return LinkResolution::Missing;
        }
        let mut matches = self.files.keys().filter(|path| {
            path.file_stem()
                .and_then(|part| part.to_str())
                .is_some_and(|part| part.eq_ignore_ascii_case(target))
        });
        match (matches.next(), matches.next()) {
            (Some(path), None) => LinkResolution::Found(path.clone()),
            (Some(_), Some(_)) => LinkResolution::Ambiguous,
            _ => LinkResolution::Missing,
        }
    }

    fn read_latest(&self, path: &Path) -> Result<Option<String>, String> {
        Ok(self.files.get(path).cloned())
    }
}

pub(super) fn test_settings() -> TasksSettings {
    TasksSettings {
        global_filter: "#task".to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::Todo),
            ('?', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: Vec::new(),
        status_settings_error: None,
    }
}
