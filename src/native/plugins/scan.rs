use std::{
    collections::{BTreeSet, HashSet},
    fs, io,
    path::Path,
};

use super::{
    deploy::{check_plugin_ignore, scan_deployment},
    model::{
        Manifest, PluginEntry, PluginsReport, RepoSource, SyncState,
        VaultState, COMMUNITY_PLUGINS_FILE, MANAGED_FILES, REPO_PLUGINS_SUBDIR,
        VAULT_PLUGINS_SUBDIR,
    },
};

pub(super) fn scan_plugins(
    repo: &Path,
    bob_dir: &Path,
    repo_source: RepoSource,
    configured: Option<&BTreeSet<String>>,
) -> PluginsReport {
    let mut plugins = Vec::new();
    let mut issues = Vec::new();
    let plugins_root = repo.join(REPO_PLUGINS_SUBDIR);
    let enabled = read_enabled_plugins(bob_dir);

    let entries = match read_sorted_directory(&plugins_root) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(format!(
                "failed to read plugins directory {}: {error}",
                plugins_root.display()
            ));
            return PluginsReport {
                repo: repo.to_path_buf(),
                repo_source,
                bob_dir: bob_dir.to_path_buf(),
                plugins,
                issues,
            };
        }
    };

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
                issues.push(format!("{folder}: {error}"));
                continue;
            }
        };

        let id = if manifest.id.is_empty() {
            folder.to_string()
        } else {
            manifest.id
        };
        let vault_plugin_dir = bob_dir.join(VAULT_PLUGINS_SUBDIR).join(&id);

        let vault_ignore = check_plugin_ignore(bob_dir, &id);
        let is_configured =
            configured.is_some_and(|allowed| allowed.contains(&id));
        plugins.push(PluginEntry {
            sync: sync_state(&path, &vault_plugin_dir),
            vault: vault_state(&id, &enabled, &vault_plugin_dir),
            deployment: scan_deployment(&vault_plugin_dir),
            vault_ignore,
            version: manifest.version,
            description: manifest.description,
            id,
            configured: is_configured,
        });
    }

    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    PluginsReport {
        repo: repo.to_path_buf(),
        repo_source,
        bob_dir: bob_dir.to_path_buf(),
        plugins,
        issues,
    }
}

pub(super) fn read_manifest(plugin_dir: &Path) -> Result<Manifest, String> {
    let manifest_path = plugin_dir.join("manifest.json");
    let contents = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read manifest.json: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("failed to parse manifest.json: {error}"))
}

/// Plugin id for one repo folder without failing on manifest problems.
pub(super) fn read_plugin_id(plugin_dir: &Path) -> String {
    let folder = plugin_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("plugin")
        .to_string();
    match read_manifest(plugin_dir) {
        Ok(manifest) if !manifest.id.is_empty() => manifest.id,
        _ => folder,
    }
}

/// Every plugin id in the current checkout, sorted. Empty when unreadable.
pub(super) fn plugin_ids_in_repo(repo: &Path) -> Vec<String> {
    let Ok(entries) = read_sorted_directory(&repo.join(REPO_PLUGINS_SUBDIR))
    else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for entry in entries {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()).is_none() {
            continue;
        }
        ids.push(read_plugin_id(&path));
    }
    ids.sort();
    ids
}

fn read_enabled_plugins(bob_dir: &Path) -> HashSet<String> {
    let path = bob_dir.join(COMMUNITY_PLUGINS_FILE);
    let Ok(contents) = fs::read_to_string(&path) else {
        return HashSet::new();
    };
    serde_json::from_str::<Vec<String>>(&contents)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub(super) fn sync_state(
    repo_plugin_dir: &Path,
    vault_plugin_dir: &Path,
) -> SyncState {
    if !vault_plugin_dir.is_dir() {
        return SyncState::Missing;
    }

    for file in MANAGED_FILES {
        let repo_file = repo_plugin_dir.join(file);
        if !repo_file.is_file() {
            continue;
        }
        let vault_file = vault_plugin_dir.join(file);
        match (fs::read(&repo_file), fs::read(&vault_file)) {
            (Ok(repo_bytes), Ok(vault_bytes)) if repo_bytes == vault_bytes => {}
            _ => return SyncState::Drift,
        }
    }

    SyncState::Synced
}

pub(super) fn vault_state(
    id: &str,
    enabled: &HashSet<String>,
    vault_plugin_dir: &Path,
) -> VaultState {
    if enabled.contains(id) {
        VaultState::Enabled
    } else if vault_plugin_dir.is_dir() {
        VaultState::Disabled
    } else {
        VaultState::NotInstalled
    }
}

pub(super) fn read_sorted_directory(
    directory: &Path,
) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}
