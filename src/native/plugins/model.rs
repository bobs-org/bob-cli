use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(super) const COMMAND_NAME: &str = "bob plugins";
pub(super) const REPO_PLUGINS_SUBDIR: &str = "plugins";
pub(super) const VAULT_PLUGINS_SUBDIR: &str = ".obsidian/plugins";
pub(super) const COMMUNITY_PLUGINS_FILE: &str =
    ".obsidian/community-plugins.json";
/// Files the repo owns and `bob plugins sync` deploys; never `data.json`.
pub(super) const MANAGED_FILES: &[&str] =
    &["manifest.json", "main.js", "styles.css"];

/// Resolved inputs for a single `bob plugins sync` invocation.
#[derive(Debug)]
pub(super) struct SyncOptions {
    pub(super) repo: PathBuf,
    pub(super) bob_dir: PathBuf,
    pub(super) backup_run_dir: PathBuf,
    pub(super) only: Option<String>,
    pub(super) dry_run: bool,
    pub(super) force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncReport {
    pub(super) repo: PathBuf,
    pub(super) bob_dir: PathBuf,
    pub(super) backup_run_dir: PathBuf,
    pub(super) plugins: Vec<PluginSync>,
    pub(super) issues: Vec<String>,
}

impl SyncReport {
    fn files(&self) -> impl Iterator<Item = &FileSync> {
        self.plugins.iter().flat_map(|plugin| plugin.files.iter())
    }

    pub(super) fn copied(&self) -> usize {
        self.files().filter(|file| file.action.is_copy()).count()
    }

    pub(super) fn skipped(&self) -> usize {
        self.files()
            .filter(|file| file.action == FileAction::SkippedDirty)
            .count()
    }

    pub(super) fn unchanged(&self) -> usize {
        self.files()
            .filter(|file| file.action == FileAction::Unchanged)
            .count()
    }

    pub(super) fn has_backup_paths(&self) -> bool {
        self.files().any(|file| file.backup.is_some())
    }

    pub(super) fn has_written_backups(&self) -> bool {
        self.files().any(|file| {
            file.backup.as_ref().is_some_and(|backup| backup.written)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PluginSync {
    pub(super) id: String,
    pub(super) files: Vec<FileSync>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileSync {
    pub(super) name: String,
    pub(super) action: FileAction,
    pub(super) diff: Option<FileDiff>,
    pub(super) backup: Option<BackupOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileDiff {
    Text {
        lines: Vec<DiffLine>,
        added: usize,
        removed: usize,
        hidden: usize,
    },
    Binary {
        old_len: usize,
        new_len: usize,
    },
    NewFile {
        lines: usize,
        bytes: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffLine {
    pub(super) kind: DiffKind,
    pub(super) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffKind {
    Hunk,
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackupOutcome {
    pub(super) path: PathBuf,
    pub(super) written: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileAction {
    /// The vault had no copy of the file; it was created.
    Created,
    /// The vault copy differed and was clean in Git; it was overwritten.
    Updated,
    /// The vault copy was dirty in Git and was overwritten because of --force.
    Forced,
    /// The vault copy already matched the repo byte-for-byte.
    Unchanged,
    /// The vault copy was dirty in Git and was left alone without --force.
    SkippedDirty,
    /// Reading or writing the file failed; the cause is recorded as an issue.
    Failed,
}

impl FileAction {
    pub(super) fn is_copy(self) -> bool {
        matches!(self, Self::Created | Self::Updated | Self::Forced)
    }

    pub(super) fn is_warning(self) -> bool {
        matches!(self, Self::SkippedDirty | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PluginsReport {
    pub(super) repo: PathBuf,
    pub(super) bob_dir: PathBuf,
    pub(super) plugins: Vec<PluginEntry>,
    pub(super) issues: Vec<String>,
}

impl PluginsReport {
    pub(super) fn counts(&self) -> StateCounts {
        let mut counts = StateCounts::default();
        for plugin in &self.plugins {
            match plugin.sync {
                SyncState::Synced => counts.synced += 1,
                SyncState::Drift => counts.drift += 1,
                SyncState::Missing => counts.not_installed += 1,
            }
        }
        counts
    }

    pub(super) fn result(&self) -> PluginsResult {
        let counts = self.counts();
        PluginsResult {
            ok: true,
            repo: self.repo.display().to_string(),
            bob_dir: self.bob_dir.display().to_string(),
            count: self.plugins.len(),
            synced: counts.synced,
            drift: counts.drift,
            not_installed: counts.not_installed,
            plugins: self.plugins.clone(),
        }
    }

    pub(super) fn issue_summary(&self) -> String {
        self.issues.join("; ")
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct StateCounts {
    pub(super) synced: usize,
    pub(super) drift: usize,
    pub(super) not_installed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PluginsResult {
    pub(super) ok: bool,
    pub(super) repo: String,
    pub(super) bob_dir: String,
    pub(super) count: usize,
    pub(super) synced: usize,
    pub(super) drift: usize,
    pub(super) not_installed: usize,
    pub(super) plugins: Vec<PluginEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PluginEntry {
    pub(super) id: String,
    pub(super) version: String,
    pub(super) description: String,
    pub(super) sync: SyncState,
    pub(super) vault: VaultState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SyncState {
    Synced,
    Drift,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum VaultState {
    Enabled,
    Disabled,
    NotInstalled,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Manifest {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) version: String,
    #[serde(default)]
    pub(super) description: String,
}
