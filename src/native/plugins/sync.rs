use std::{fs, path::Path, path::PathBuf};

use super::{
    deploy::{
        allocate_backup_run_dir, backup_prior_stamp, build_stamp,
        capture_plugin, check_plugin_ignore, check_stamp_storage,
        current_agent, current_host, deploy_timestamp, deployment_info,
        deployment_info_from_prior, evaluate_guard, read_prior_stamp,
        snapshot_vault, stamp_path, vault_bytes, verify_copied_bytes,
        write_stamp_atomic, CapturedPlugin, DeployLock, GuardDecision,
        PriorStamp, StampAction, StampState, VaultBundle, STAMP_FILE,
    },
    diff::{byte_line_count, diff_existing_file},
    git::vault_file_is_dirty,
    model::{
        BackupOutcome, FileAction, FileDiff, FileSync, PluginSync, SyncOptions,
        SyncReport, REPO_PLUGINS_SUBDIR,
    },
    scan::{read_manifest, read_sorted_directory},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileOutcome {
    action: FileAction,
    diff: Option<FileDiff>,
    backup: Option<BackupOutcome>,
    issue: Option<String>,
    repo_bytes: Option<Vec<u8>>,
}

pub(super) fn sync_plugins(options: &SyncOptions) -> SyncReport {
    let mut report = SyncReport {
        repo: options.repo.clone(),
        bob_dir: options.bob_dir.clone(),
        backup_run_dir: options.backup_run_dir.clone(),
        plugins: Vec::new(),
        issues: Vec::new(),
    };

    let _lock = if options.dry_run {
        None
    } else {
        match acquire_lock(&options.bob_dir, &mut report.issues) {
            Some(lock) => Some(lock),
            None => return report,
        }
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

    let host = current_host();
    let agent = current_agent();
    let deployed_at = deploy_timestamp();
    let mut backup_run_dir = options.backup_run_dir.clone();
    let mut backup_dir_allocated = false;

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

        let plugin = sync_one_plugin(
            options,
            &id,
            &path,
            &host,
            agent.clone(),
            &deployed_at,
            &mut backup_run_dir,
            &mut backup_dir_allocated,
            &mut report.issues,
        );
        report.plugins.push(plugin);
    }

    if let Some(only) = &options.only
        && !matched
    {
        report
            .issues
            .push(format!("plugin not found in repo: {only}"));
    }

    report.backup_run_dir = backup_run_dir;
    report.plugins.sort_by(|left, right| left.id.cmp(&right.id));
    report
}

fn acquire_lock(
    bob_dir: &Path,
    issues: &mut Vec<String>,
) -> Option<DeployLock> {
    match super::deploy::acquire_deploy_lock(bob_dir) {
        Ok(lock) => Some(lock),
        Err(message) => {
            issues.push(message);
            None
        }
    }
}

fn sync_one_plugin(
    options: &SyncOptions,
    id: &str,
    repo_plugin_dir: &Path,
    host: &str,
    agent: Option<String>,
    deployed_at: &str,
    backup_run_dir: &mut PathBuf,
    backup_dir_allocated: &mut bool,
    issues: &mut Vec<String>,
) -> PluginSync {
    let vault_plugin_dir = options
        .bob_dir
        .join(super::model::VAULT_PLUGINS_SUBDIR)
        .join(id);

    // A vault-ignore warning never blocks: it rides along on every outcome,
    // including dry-run previews and stamp/guard refusals.
    let vault_ignore = check_plugin_ignore(&options.bob_dir, id);

    if !options.dry_run
        && let Err(message) = check_stamp_storage(&options.bob_dir, id)
    {
        issues.push(message.clone());
        return PluginSync {
            id: id.to_string(),
            files: Vec::new(),
            deployment: super::deploy::scan_deployment(&vault_plugin_dir),
            vault_ignore,
            blocked: Some(message),
        };
    }

    let captured = match capture_plugin(&options.repo, repo_plugin_dir, id) {
        Ok(captured) => captured,
        Err(message) => {
            issues.push(message.clone());
            return PluginSync {
                id: id.to_string(),
                files: Vec::new(),
                deployment: super::deploy::scan_deployment(&vault_plugin_dir),
                vault_ignore,
                blocked: Some(message),
            };
        }
    };

    let vault = snapshot_vault(&vault_plugin_dir);
    let stamp_file = stamp_path(&options.bob_dir, id);
    let prior = read_prior_stamp(&stamp_file, &vault);
    let decision = evaluate_guard(
        &prior,
        &captured,
        &vault,
        host,
        options.force,
        &options.repo,
        id,
    );

    match decision {
        GuardDecision::Refuse { message } => {
            issues.push(message.clone());
            PluginSync {
                id: id.to_string(),
                files: Vec::new(),
                deployment: deployment_info_from_prior(&prior),
                vault_ignore,
                blocked: Some(message),
            }
        }
        GuardDecision::Permit { stamp_action } => apply_plugin(
            options,
            id,
            &captured,
            &vault,
            &vault_plugin_dir,
            &stamp_file,
            &prior,
            stamp_action,
            host,
            agent,
            deployed_at,
            backup_run_dir,
            backup_dir_allocated,
            vault_ignore,
            issues,
        ),
    }
}

fn apply_plugin(
    options: &SyncOptions,
    id: &str,
    captured: &CapturedPlugin,
    vault: &VaultBundle,
    vault_plugin_dir: &Path,
    stamp_file: &Path,
    prior: &PriorStamp,
    stamp_action: StampAction,
    host: &str,
    agent: Option<String>,
    deployed_at: &str,
    backup_run_dir: &mut PathBuf,
    backup_dir_allocated: &mut bool,
    vault_ignore: Option<super::deploy::VaultIgnoreInfo>,
    issues: &mut Vec<String>,
) -> PluginSync {
    let mut files = Vec::new();
    let mut outcomes = Vec::new();
    for asset in &captured.assets {
        let Some(repo_bytes) = &asset.bytes else {
            continue;
        };
        let vault_file = vault_plugin_dir.join(asset.name);
        let backup_file = backup_run_dir.join(id).join(asset.name);
        let outcome = plan_one_file(
            options,
            asset.name,
            repo_bytes,
            vault_bytes(vault, asset.name),
            vault_file.exists(),
            &vault_file,
            &backup_file,
        );
        if let Some(issue) = &outcome.issue {
            issues.push(format!("{id}/{}: {issue}", asset.name));
        }
        outcomes.push((asset.name, vault_file, backup_file, outcome));
    }

    let any_copy = outcomes
        .iter()
        .any(|(_, _, _, outcome)| outcome.action.is_copy());
    let any_fail = outcomes
        .iter()
        .any(|(_, _, _, outcome)| outcome.action == FileAction::Failed);
    let replace_protected = options.force
        && matches!(
            prior,
            PriorStamp::Complete { .. }
                | PriorStamp::Pending { .. }
                | PriorStamp::Malformed(_)
                | PriorStamp::Unsupported(_)
        );

    if !options.dry_run && any_copy && !any_fail {
        if replace_protected {
            if let Err(message) =
                ensure_backup_dir(options, backup_run_dir, backup_dir_allocated)
            {
                issues.push(format!("{id}: {message}"));
                return PluginSync {
                    id: id.to_string(),
                    files: Vec::new(),
                    deployment: deployment_info_from_prior(prior),
                    vault_ignore: vault_ignore.clone(),
                    blocked: Some(message),
                };
            }
            let stamp_backup = backup_run_dir.join(id).join(STAMP_FILE);
            if let Err(message) = backup_prior_stamp(stamp_file, &stamp_backup)
            {
                issues.push(format!("{id}: {message}"));
                return PluginSync {
                    id: id.to_string(),
                    files: Vec::new(),
                    deployment: deployment_info_from_prior(prior),
                    vault_ignore: vault_ignore.clone(),
                    blocked: Some(message),
                };
            }
        }
        if let Err(message) = write_stamp_atomic(
            stamp_file,
            &build_stamp(
                id,
                captured,
                StampState::Pending,
                host,
                deployed_at,
                agent.clone(),
            ),
        ) {
            issues.push(format!("{id}: {message}"));
            return PluginSync {
                id: id.to_string(),
                files: Vec::new(),
                deployment: deployment_info_from_prior(prior),
                vault_ignore: vault_ignore.clone(),
                blocked: Some(message),
            };
        }
    }

    for (name, vault_file, _backup_file, mut outcome) in outcomes {
        if !options.dry_run
            && outcome.action.is_copy()
            && outcome.issue.is_none()
        {
            if let Err(message) =
                ensure_backup_dir(options, backup_run_dir, backup_dir_allocated)
            {
                outcome.action = FileAction::Failed;
                outcome.issue = Some(message);
                issues.push(format!(
                    "{id}/{name}: {}",
                    outcome.issue.as_ref().unwrap()
                ));
            } else {
                let backup_file = backup_run_dir.join(id).join(name);
                outcome.backup = outcome.backup.map(|backup| BackupOutcome {
                    path: backup_file.clone(),
                    written: backup.written,
                });
                match apply_one_file(
                    &outcome,
                    vault_file.as_path(),
                    backup_file.as_path(),
                ) {
                    Ok(applied) => outcome = applied,
                    Err(message) => {
                        issues.push(format!("{id}/{name}: {message}"));
                        outcome.action = FileAction::Failed;
                    }
                }
            }
        }
        files.push(FileSync {
            name: name.to_string(),
            action: outcome.action,
            diff: outcome.diff,
            backup: outcome.backup,
        });
    }

    let copied_ok = files.iter().any(|file| file.action.is_copy())
        && files.iter().all(|file| {
            file.action != FileAction::Failed
                && file.action != FileAction::SkippedDirty
        });
    let skip_or_fail = files.iter().any(|file| {
        file.action == FileAction::Failed
            || file.action == FileAction::SkippedDirty
    });
    let complete_stamp = build_stamp(
        id,
        captured,
        StampState::Complete,
        host,
        deployed_at,
        agent.clone(),
    );
    let pending_stamp = build_stamp(
        id,
        captured,
        StampState::Pending,
        host,
        deployed_at,
        agent,
    );

    let mut deployment = deployment_info_from_prior(prior);
    if options.dry_run {
        if stamp_action == StampAction::Write {
            deployment = Some(deployment_info(&complete_stamp));
        }
        return PluginSync {
            id: id.to_string(),
            files,
            deployment,
            vault_ignore,
            blocked: None,
        };
    }

    if any_copy && copied_ok {
        if let Err(message) = verify_copied_bytes(vault_plugin_dir, captured) {
            issues.push(format!("{id}: {message}"));
            deployment = Some(deployment_info(&pending_stamp));
        } else if let Err(message) =
            write_stamp_atomic(stamp_file, &complete_stamp)
        {
            issues.push(format!("{id}: {message}"));
            deployment = Some(deployment_info(&pending_stamp));
        } else {
            deployment = Some(deployment_info(&complete_stamp));
        }
    } else if !any_copy && !skip_or_fail && stamp_action == StampAction::Write {
        if let Err(message) = write_stamp_atomic(stamp_file, &complete_stamp) {
            issues.push(format!("{id}: {message}"));
        } else {
            deployment = Some(deployment_info(&complete_stamp));
        }
    } else if any_copy && skip_or_fail {
        deployment = Some(deployment_info(&pending_stamp));
    }

    PluginSync {
        id: id.to_string(),
        files,
        deployment,
        vault_ignore,
        blocked: None,
    }
}

fn ensure_backup_dir(
    options: &SyncOptions,
    backup_run_dir: &mut PathBuf,
    allocated: &mut bool,
) -> Result<(), String> {
    if options.dry_run || *allocated {
        return Ok(());
    }
    *backup_run_dir = allocate_backup_run_dir(backup_run_dir)?;
    *allocated = true;
    Ok(())
}

fn plan_one_file(
    options: &SyncOptions,
    name: &str,
    repo_bytes: &[u8],
    vault_bytes: Option<&[u8]>,
    vault_exists: bool,
    vault_file: &Path,
    backup_file: &Path,
) -> FileOutcome {
    let mut dirty = false;
    let diff;
    if vault_exists {
        let Some(vault_bytes) = vault_bytes else {
            return FileOutcome {
                action: FileAction::Failed,
                diff: None,
                backup: None,
                issue: Some(format!("failed to read vault file {name}")),
                repo_bytes: Some(repo_bytes.to_vec()),
            };
        };
        if vault_bytes == repo_bytes {
            return FileOutcome {
                action: FileAction::Unchanged,
                diff: None,
                backup: None,
                issue: None,
                repo_bytes: Some(repo_bytes.to_vec()),
            };
        }
        diff = Some(diff_existing_file(vault_bytes, repo_bytes));
        dirty = vault_file_is_dirty(&options.bob_dir, vault_file);
        if dirty && !options.force {
            return FileOutcome {
                action: FileAction::SkippedDirty,
                diff,
                backup: None,
                issue: None,
                repo_bytes: Some(repo_bytes.to_vec()),
            };
        }
    } else {
        diff = Some(FileDiff::NewFile {
            lines: byte_line_count(repo_bytes),
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

    let backup = if matches!(action, FileAction::Updated | FileAction::Forced) {
        Some(BackupOutcome {
            path: backup_file.to_path_buf(),
            written: false,
        })
    } else {
        None
    };

    FileOutcome {
        action,
        diff,
        backup,
        issue: None,
        repo_bytes: Some(repo_bytes.to_vec()),
    }
}

fn apply_one_file(
    planned: &FileOutcome,
    vault_file: &Path,
    backup_file: &Path,
) -> Result<FileOutcome, String> {
    let mut outcome = planned.clone();
    let repo_bytes = outcome
        .repo_bytes
        .as_deref()
        .ok_or_else(|| "missing captured bytes for vault write".to_string())?;

    if matches!(outcome.action, FileAction::Updated | FileAction::Forced) {
        if let Some(parent) = backup_file.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            outcome.action = FileAction::Failed;
            outcome.issue = Some(format!(
                "failed to create backup directory {}: {error}",
                parent.display()
            ));
            return Ok(outcome);
        }
        if let Err(error) = fs::copy(vault_file, backup_file) {
            outcome.action = FileAction::Failed;
            outcome.issue = Some(format!(
                "failed to back up vault file to {}: {error}",
                backup_file.display()
            ));
            return Ok(outcome);
        }
        outcome.backup = Some(BackupOutcome {
            path: backup_file.to_path_buf(),
            written: true,
        });
    }

    if let Some(parent) = vault_file.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        outcome.action = FileAction::Failed;
        outcome.issue =
            Some(format!("failed to create vault directory: {error}"));
        return Ok(outcome);
    }
    if let Err(error) = fs::write(vault_file, repo_bytes) {
        outcome.action = FileAction::Failed;
        outcome.issue = Some(format!("failed to write vault file: {error}"));
        return Ok(outcome);
    }

    Ok(outcome)
}
