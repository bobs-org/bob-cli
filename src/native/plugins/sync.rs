use std::{fs, path::Path};

use super::{
    diff::{byte_line_count, diff_existing_file},
    git::vault_file_is_dirty,
    model::{
        BackupOutcome, FileAction, FileDiff, FileSync, PluginSync, SyncOptions,
        SyncReport, MANAGED_FILES, REPO_PLUGINS_SUBDIR,
    },
    scan::{read_manifest, read_sorted_directory},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileOutcome {
    action: FileAction,
    diff: Option<FileDiff>,
    backup: Option<BackupOutcome>,
    issue: Option<String>,
}

pub(super) fn sync_plugins(options: &SyncOptions) -> SyncReport {
    let mut report = SyncReport {
        repo: options.repo.clone(),
        bob_dir: options.bob_dir.clone(),
        backup_run_dir: options.backup_run_dir.clone(),
        plugins: Vec::new(),
        issues: Vec::new(),
    };

    let plugins_root = options.repo.join(REPO_PLUGINS_SUBDIR);
    let entries = match read_sorted_directory(&plugins_root) {
        Ok(entries) => entries,
        Err(error) => {
            report.issues.push(format!(
                "failed to read plugins directory {}: {error}",
                plugins_root.display()
            ));
            return report;
        }
    };

    let mut matched = false;
    for entry in entries {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(folder) = path.file_name().and_then(|name| name.to_str())
        else {
            continue;
        };

        let manifest = match read_manifest(&path) {
            Ok(manifest) => manifest,
            Err(error) => {
                report.issues.push(format!("{folder}: {error}"));
                continue;
            }
        };

        let id = if manifest.id.is_empty() {
            folder.to_string()
        } else {
            manifest.id
        };
        if options.only.as_deref().is_some_and(|only| only != id) {
            continue;
        }
        matched = true;

        let plugin = sync_one_plugin(options, &id, &path, &mut report.issues);
        report.plugins.push(plugin);
    }

    if let Some(only) = &options.only
        && !matched
    {
        report
            .issues
            .push(format!("plugin not found in repo: {only}"));
    }

    report.plugins.sort_by(|left, right| left.id.cmp(&right.id));
    report
}

fn sync_one_plugin(
    options: &SyncOptions,
    id: &str,
    repo_plugin_dir: &Path,
    issues: &mut Vec<String>,
) -> PluginSync {
    let vault_plugin_dir = options
        .bob_dir
        .join(super::model::VAULT_PLUGINS_SUBDIR)
        .join(id);
    let mut files = Vec::new();

    for &name in MANAGED_FILES {
        let repo_file = repo_plugin_dir.join(name);
        if !repo_file.is_file() {
            continue;
        }
        let vault_file = vault_plugin_dir.join(name);
        let backup_file = options.backup_run_dir.join(id).join(name);
        let outcome =
            match sync_one_file(options, &repo_file, &vault_file, &backup_file)
            {
                Ok(outcome) => {
                    if let Some(issue) = &outcome.issue {
                        issues.push(format!("{id}/{name}: {issue}"));
                    }
                    outcome
                }
                Err(message) => {
                    issues.push(format!("{id}/{name}: {message}"));
                    FileOutcome {
                        action: FileAction::Failed,
                        diff: None,
                        backup: None,
                        issue: None,
                    }
                }
            };
        files.push(FileSync {
            name: name.to_string(),
            action: outcome.action,
            diff: outcome.diff,
            backup: outcome.backup,
        });
    }

    PluginSync {
        id: id.to_string(),
        files,
    }
}

fn sync_one_file(
    options: &SyncOptions,
    repo_file: &Path,
    vault_file: &Path,
    backup_file: &Path,
) -> Result<FileOutcome, String> {
    let repo_bytes = fs::read(repo_file)
        .map_err(|error| format!("failed to read repo file: {error}"))?;

    let vault_exists = vault_file.is_file();
    let mut dirty = false;
    let diff;
    if vault_exists {
        let vault_bytes = fs::read(vault_file)
            .map_err(|error| format!("failed to read vault file: {error}"))?;
        if vault_bytes == repo_bytes {
            return Ok(FileOutcome {
                action: FileAction::Unchanged,
                diff: None,
                backup: None,
                issue: None,
            });
        }
        diff = Some(diff_existing_file(&vault_bytes, &repo_bytes));
        dirty = vault_file_is_dirty(&options.bob_dir, vault_file);
        if dirty && !options.force {
            return Ok(FileOutcome {
                action: FileAction::SkippedDirty,
                diff,
                backup: None,
                issue: None,
            });
        }
    } else {
        diff = Some(FileDiff::NewFile {
            lines: byte_line_count(&repo_bytes),
            bytes: repo_bytes.len(),
        });
    }

    let action = if !vault_exists {
        FileAction::Created
    } else if dirty {
        FileAction::Forced
    } else {
        FileAction::Updated
    };

    let mut backup = None;
    if matches!(action, FileAction::Updated | FileAction::Forced) {
        backup = Some(BackupOutcome {
            path: backup_file.to_path_buf(),
            written: false,
        });
        if !options.dry_run {
            if let Some(parent) = backup_file.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return Ok(FileOutcome {
                        action: FileAction::Failed,
                        diff,
                        backup,
                        issue: Some(format!(
                            "failed to create backup directory {}: {error}",
                            parent.display()
                        )),
                    });
                }
            }
            if let Err(error) = fs::copy(vault_file, backup_file) {
                return Ok(FileOutcome {
                    action: FileAction::Failed,
                    diff,
                    backup,
                    issue: Some(format!(
                        "failed to back up vault file to {}: {error}",
                        backup_file.display()
                    )),
                });
            }
            backup = Some(BackupOutcome {
                path: backup_file.to_path_buf(),
                written: true,
            });
        }
    }

    if !options.dry_run {
        if let Some(parent) = vault_file.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return Ok(FileOutcome {
                    action: FileAction::Failed,
                    diff,
                    backup,
                    issue: Some(format!(
                        "failed to create vault directory: {error}"
                    )),
                });
            }
        }
        if let Err(error) = fs::write(vault_file, &repo_bytes) {
            return Ok(FileOutcome {
                action: FileAction::Failed,
                diff,
                backup,
                issue: Some(format!("failed to write vault file: {error}")),
            });
        }
    }

    Ok(FileOutcome {
        action,
        diff,
        backup,
        issue: None,
    })
}
