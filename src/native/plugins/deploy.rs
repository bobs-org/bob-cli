//! Deployment provenance stamps and overwrite protection for plugin sync.
//!
//! Filesystem and Git probes live here separately from the refusal policy
//! so sync can decide before copying any file of a plugin.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::Command,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::native::{env as bob_env, ob, vault_git, vault_sync};

use super::model::{MANAGED_FILES, VAULT_PLUGINS_SUBDIR};

pub(super) const STAMP_FILE: &str = ".bob-deploy.json";
pub(super) const STAMP_VERSION: u32 = 1;
pub(super) const STAMP_IGNORE_HINT: &str =
    ".obsidian/plugins/*/.bob-deploy.json";

/// Structured deployment provenance for list/sync JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DeploymentInfo {
    pub(super) status: DeployStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) deployed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeployStatus {
    Clean,
    Uncommitted,
    Unknown,
    Incomplete,
    Mismatched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DeployStamp {
    pub(super) version: u32,
    pub(super) plugin_id: String,
    pub(super) source_root: String,
    #[serde(default)]
    pub(super) source_head: Option<String>,
    pub(super) dirty: DirtyState,
    pub(super) host: String,
    pub(super) deployed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) agent: Option<String>,
    pub(super) assets: Vec<AssetFingerprint>,
    pub(super) digest: String,
    pub(super) state: StampState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DirtyState {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StampState {
    Pending,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AssetFingerprint {
    pub(super) name: String,
    pub(super) present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceMeta {
    pub(super) root: PathBuf,
    pub(super) head: Option<String>,
    pub(super) dirty: DirtyState,
    pub(super) kind: SourceGitKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceGitKind {
    Worktree,
    NonGit,
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapturedPlugin {
    pub(super) meta: SourceMeta,
    pub(super) assets: Vec<CapturedAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapturedAsset {
    pub(super) name: &'static str,
    pub(super) bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VaultBundle {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PriorStamp {
    Missing,
    Complete {
        stamp: DeployStamp,
        matches_vault: bool,
    },
    Pending {
        stamp: DeployStamp,
        matches_vault: bool,
    },
    Malformed(String),
    Unsupported(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GuardDecision {
    Permit { stamp_action: StampAction },
    Refuse { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StampAction {
    Leave,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ancestor {
    Yes,
    No,
    Unknown,
}

pub(super) struct DeployLock {
    file: File,
}

impl Drop for DeployLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn stamp_path(bob_dir: &Path, plugin_id: &str) -> PathBuf {
    bob_dir
        .join(VAULT_PLUGINS_SUBDIR)
        .join(plugin_id)
        .join(STAMP_FILE)
}

pub(super) fn stamp_rel_path(plugin_id: &str) -> PathBuf {
    PathBuf::from(VAULT_PLUGINS_SUBDIR)
        .join(plugin_id)
        .join(STAMP_FILE)
}

pub(super) fn current_host() -> String {
    vault_sync::hostname_slug()
}

pub(super) fn current_agent() -> Option<String> {
    bob_env::var("SASE_AGENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The stamp's `deployed_at` in real UTC, so the `Z` suffix is true on
/// every host. `BOB_NOW` pins Bob's day-zone clock (tests), so a pinned
/// time converts through the configured zone.
pub(super) fn deploy_timestamp() -> String {
    use chrono::{Local, TimeZone, Utc};
    let pinned = bob_env::var("BOB_NOW")
        .ok()
        .and_then(|value| bob_env::parse_datetime_override(&value));
    let now = pinned
        .and_then(|local| match bob_env::configured_timezone() {
            Some(zone) => zone
                .from_local_datetime(&local)
                .earliest()
                .map(|time| time.with_timezone(&Utc)),
            None => Local
                .from_local_datetime(&local)
                .earliest()
                .map(|time| time.with_timezone(&Utc)),
        })
        .unwrap_or_else(Utc::now);
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(super) fn canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    path.to_path_buf()
}

pub(super) fn acquire_deploy_lock(
    bob_dir: &Path,
) -> Result<DeployLock, String> {
    let path = deploy_lock_path(bob_dir);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file = bob_env::create_state_lock_file(
        parent,
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("deploy.lock"),
    )
    .map_err(|error| {
        format!(
            "failed to open plugin deploy lock {}: {error}",
            path.display()
        )
    })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(DeployLock { file }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err(format!(
                "another bob plugins sync is already deploying to this vault; \
retry shortly ({})",
                path.display()
            ))
        }
        Err(error) => Err(format!(
            "failed to lock plugin deploy lock {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn deploy_lock_path(bob_dir: &Path) -> PathBuf {
    let identity = canonical_path(bob_dir);
    let digest = sha256_hex(identity.to_string_lossy().as_bytes());
    bob_env::bob_cli_state_dir()
        .join("plugins")
        .join(format!("deploy-{digest}.lock"))
}

pub(super) fn allocate_backup_run_dir(
    backup_run_dir: &Path,
) -> Result<PathBuf, String> {
    let Some(parent) = backup_run_dir.parent() else {
        return Ok(backup_run_dir.to_path_buf());
    };
    let Some(name) = backup_run_dir.file_name().and_then(|n| n.to_str()) else {
        return Ok(backup_run_dir.to_path_buf());
    };
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create backup directory {}: {error}",
            parent.display()
        )
    })?;
    let mut n = 0u32;
    loop {
        let candidate = if n == 0 {
            parent.join(name)
        } else {
            parent.join(format!("{name}-{n}"))
        };
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !candidate.is_dir() {
                    return Err(format!(
                        "failed to create backup directory {}: {error}",
                        candidate.display()
                    ));
                }
                n = n.saturating_add(1);
                if n > 10_000 {
                    return Err(format!(
                        "failed to allocate a unique backup directory under {}",
                        parent.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "failed to create backup directory {}: {error}",
                    candidate.display()
                ));
            }
        }
    }
}

/// Refuse real copies unless the stamp path stays out of vault Git.
///
/// `--force` does not bypass this. A non-Git vault needs no ignore rule.
pub(super) fn check_stamp_storage(
    bob_dir: &Path,
    plugin_id: &str,
) -> Result<(), String> {
    match inspect_vault_git(bob_dir) {
        VaultGit::NonGit => Ok(()),
        VaultGit::ProbeFailed(message) => Err(message),
        VaultGit::Git => {
            let rel = stamp_rel_path(plugin_id);
            let child_env = ob::child_env();
            if stamp_is_tracked(bob_dir, &rel) {
                return Err(format!(
                    "refusing to deploy {plugin_id}: deploy stamp {} is \
tracked by vault Git. Remove it from the index and add `{STAMP_IGNORE_HINT}` \
to the vault .gitignore so provenance stays out of version control. --force \
does not bypass this.",
                    rel.display()
                ));
            }
            match vault_git::check_ignore_quiet(bob_dir, &child_env, &rel) {
                Ok(vault_git::IgnoreQuiet::Ignored) => Ok(()),
                Ok(vault_git::IgnoreQuiet::Trackable) => Err(format!(
                    "refusing to deploy {plugin_id}: deploy stamp {} is not \
ignored by vault Git. Add `{STAMP_IGNORE_HINT}` to the vault .gitignore so \
provenance stays out of version control. --force does not bypass this.",
                    rel.display()
                )),
                Err(error) => Err(format!(
                    "cannot verify that {} will stay out of vault Git: {error}",
                    rel.display()
                )),
            }
        }
    }
}

/// Vault-ignore finding for one plugin's whole vault folder.
///
/// Carried by sync and list output (table and JSON) as a warning only.
/// Nothing refuses on it and nothing edits `.gitignore` or the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct VaultIgnoreInfo {
    pub(super) status: VaultIgnoreStatus,
    pub(super) warning: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum VaultIgnoreStatus {
    Tracked,
    NotIgnored,
    Unknown,
}

pub(super) enum VaultGit {
    Git,
    NonGit,
    ProbeFailed(String),
}

pub(super) fn inspect_vault_git(bob_dir: &Path) -> VaultGit {
    if !bob_dir.exists() {
        return VaultGit::NonGit;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true" =>
        {
            VaultGit::Git
        }
        Ok(output) if output.status.success() => VaultGit::NonGit,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            if stderr.contains("not a git repository")
                || stderr.contains("cannot change to")
            {
                VaultGit::NonGit
            } else {
                VaultGit::ProbeFailed(format!(
                    "cannot inspect vault Git for deploy stamps: {}",
                    one_line(&output.stderr)
                ))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if has_git_ancestor(bob_dir) {
                VaultGit::ProbeFailed(
                    "cannot inspect vault Git for deploy stamps: git is \
unavailable"
                        .to_string(),
                )
            } else {
                VaultGit::NonGit
            }
        }
        Err(error) => VaultGit::ProbeFailed(format!(
            "cannot inspect vault Git for deploy stamps: {error}"
        )),
    }
}

fn stamp_is_tracked(bob_dir: &Path, rel: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("ls-files")
        .arg("--error-unmatch")
        .arg("--")
        .arg(rel)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()
        .is_some_and(|output| output.status.success())
}

/// Vault-relative directory for one plugin's whole vault folder.
pub(super) fn plugin_rel_dir(plugin_id: &str) -> PathBuf {
    PathBuf::from(VAULT_PLUGINS_SUBDIR).join(plugin_id)
}

/// Warn when a deployed plugin's whole vault folder is not git-ignored or
/// holds tracked files.
///
/// Reuses the stamp-storage Git probes (`git ls-files` / `git check-ignore`)
/// but covers the plugin folder and every managed file instead of only the
/// `.bob-deploy.json` stamp. Never refuses: callers carry the finding as a
/// warning in sync output, list output, and both JSON shapes. A non-Git
/// vault needs no ignore rule, so there is no warning. Never edits
/// `.gitignore` and never untracks files: untracking on one host deletes the
/// files on the others at their next pull.
pub(super) fn check_plugin_ignore(
    bob_dir: &Path,
    plugin_id: &str,
) -> Option<VaultIgnoreInfo> {
    let dir_rel = plugin_rel_dir(plugin_id);
    match inspect_vault_git(bob_dir) {
        VaultGit::NonGit => None,
        VaultGit::ProbeFailed(message) => Some(VaultIgnoreInfo {
            status: VaultIgnoreStatus::Unknown,
            warning: format!(
                "cannot verify vault Git ignore rules for plugin \
{plugin_id}: {message}"
            ),
        }),
        VaultGit::Git => {
            match tracked_files_under(bob_dir, &dir_rel) {
                Err(error) => {
                    return Some(VaultIgnoreInfo {
                        status: VaultIgnoreStatus::Unknown,
                        warning: format!(
                            "cannot verify vault Git ignore rules for plugin \
{plugin_id}: {error}"
                        ),
                    });
                }
                Ok(tracked) if !tracked.is_empty() => {
                    return Some(VaultIgnoreInfo {
                        status: VaultIgnoreStatus::Tracked,
                        warning: tracked_warning(plugin_id, &dir_rel, &tracked),
                    });
                }
                Ok(_) => {}
            }
            let child_env = ob::child_env();
            let mut unignored = Vec::new();
            for rel in plugin_ignore_probe_paths(plugin_id) {
                match vault_git::check_ignore_quiet(bob_dir, &child_env, &rel) {
                    Ok(vault_git::IgnoreQuiet::Ignored) => {}
                    Ok(vault_git::IgnoreQuiet::Trackable) => {
                        unignored.push(rel.display().to_string());
                    }
                    Err(error) => {
                        return Some(VaultIgnoreInfo {
                            status: VaultIgnoreStatus::Unknown,
                            warning: format!(
                                "cannot verify vault Git ignore rules for \
plugin {plugin_id}: {error}"
                            ),
                        });
                    }
                }
            }
            if unignored.is_empty() {
                None
            } else {
                Some(VaultIgnoreInfo {
                    status: VaultIgnoreStatus::NotIgnored,
                    warning: format!(
                        "plugin {plugin_id} vault folder {} is not ignored \
by vault Git ({}). Add `.obsidian/plugins/{plugin_id}/` to the vault \
.gitignore so deploys stay out of version control. bob never edits \
.gitignore.",
                        dir_rel.display(),
                        unignored.join(", "),
                    ),
                })
            }
        }
    }
}

/// The folder itself plus every managed file: all of them must be ignored.
fn plugin_ignore_probe_paths(plugin_id: &str) -> Vec<PathBuf> {
    let dir_rel = plugin_rel_dir(plugin_id);
    let mut paths = vec![dir_rel.clone()];
    for &name in MANAGED_FILES {
        paths.push(dir_rel.join(name));
    }
    paths
}

fn tracked_files_under(
    bob_dir: &Path,
    dir_rel: &Path,
) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("ls-files")
        .arg("--")
        .arg(dir_rel)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("failed to run git ls-files: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot list tracked vault files under {}: {}",
            dir_rel.display(),
            one_line(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn tracked_warning(
    plugin_id: &str,
    dir_rel: &Path,
    tracked: &[String],
) -> String {
    let shown = tracked
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let count = tracked.len();
    let files = if count == 1 {
        format!("1 tracked file ({shown})")
    } else {
        format!("{count} tracked files ({shown})")
    };
    format!(
        "plugin {plugin_id} vault folder {} has {files} in vault Git. Remove \
them from the index and add `.obsidian/plugins/{plugin_id}/` to the vault \
.gitignore so deploys stay out of version control. bob never runs `git rm \
--cached`: untracking on one host deletes the files on the others at their \
next pull.",
        dir_rel.display()
    )
}

fn has_git_ancestor(dir: &Path) -> bool {
    let mut current = Some(dir);
    while let Some(path) = current {
        let dot_git = path.join(".git");
        if dot_git.is_dir() || dot_git.is_file() {
            return true;
        }
        current = path.parent();
    }
    false
}

pub(super) fn capture_plugin(
    repo: &Path,
    plugin_dir: &Path,
    plugin_id: &str,
) -> Result<CapturedPlugin, String> {
    let before = probe_source(repo)?;
    let mut assets = Vec::new();
    for &name in MANAGED_FILES {
        let path = plugin_dir.join(name);
        let bytes = if path.is_file() {
            Some(fs::read(&path).map_err(|error| {
                format!("{plugin_id}/{name}: failed to read repo file: {error}")
            })?)
        } else {
            None
        };
        assets.push(CapturedAsset { name, bytes });
    }
    let after = probe_source(repo)?;
    if before != after {
        return Err(format!(
            "{plugin_id}: source checkout changed during capture; refuse \
rather than attribute a mixed snapshot to one revision"
        ));
    }
    Ok(CapturedPlugin {
        meta: after,
        assets,
    })
}

pub(super) fn probe_source(repo: &Path) -> Result<SourceMeta, String> {
    let child_env = ob::child_env();
    let output = ob::git_command(repo, &child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output();
    match output {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true" =>
        {
            let root =
                git_toplevel(repo).unwrap_or_else(|| canonical_path(repo));
            let head = git_head(repo);
            let dirty = git_dirty(repo);
            if matches!(dirty, DirtyState::Unknown) && head.is_none() {
                return Ok(SourceMeta {
                    root,
                    head,
                    dirty: DirtyState::Unknown,
                    kind: SourceGitKind::ProbeFailed,
                });
            }
            Ok(SourceMeta {
                root,
                head,
                dirty,
                kind: SourceGitKind::Worktree,
            })
        }
        Ok(output) if output.status.success() => Ok(non_git_source(repo)),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            if stderr.contains("not a git repository") {
                Ok(non_git_source(repo))
            } else {
                Ok(SourceMeta {
                    root: canonical_path(repo),
                    head: None,
                    dirty: DirtyState::Unknown,
                    kind: SourceGitKind::ProbeFailed,
                })
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if has_git_ancestor(repo) {
                Ok(SourceMeta {
                    root: canonical_path(repo),
                    head: None,
                    dirty: DirtyState::Unknown,
                    kind: SourceGitKind::ProbeFailed,
                })
            } else {
                Ok(non_git_source(repo))
            }
        }
        Err(_) => Ok(SourceMeta {
            root: canonical_path(repo),
            head: None,
            dirty: DirtyState::Unknown,
            kind: SourceGitKind::ProbeFailed,
        }),
    }
}

fn non_git_source(repo: &Path) -> SourceMeta {
    SourceMeta {
        root: canonical_path(repo),
        head: None,
        dirty: DirtyState::Unknown,
        kind: SourceGitKind::NonGit,
    }
}

fn git_toplevel(repo: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(canonical_path(Path::new(&text)))
    }
}

fn git_head(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("HEAD")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn git_dirty(repo: &Path) -> DirtyState {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("status")
        .arg("--porcelain")
        .arg("--untracked-files=all")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
                DirtyState::Dirty
            } else {
                DirtyState::Clean
            }
        }
        _ => DirtyState::Unknown,
    }
}

pub(super) fn snapshot_vault(vault_plugin_dir: &Path) -> VaultBundle {
    let mut files = BTreeMap::new();
    for &name in MANAGED_FILES {
        let path = vault_plugin_dir.join(name);
        if let Ok(bytes) = fs::read(&path) {
            files.insert(name.to_string(), bytes);
        }
    }
    VaultBundle { files }
}

pub(super) fn vault_bytes<'a>(
    vault: &'a VaultBundle,
    name: &str,
) -> Option<&'a [u8]> {
    vault.files.get(name).map(Vec::as_slice)
}

pub(super) fn read_prior_stamp(path: &Path, vault: &VaultBundle) -> PriorStamp {
    if !path.is_file() {
        return PriorStamp::Missing;
    }
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            return PriorStamp::Malformed(format!(
                "failed to read deploy stamp: {error}"
            ));
        }
    };
    let stamp: DeployStamp = match serde_json::from_str(&contents) {
        Ok(stamp) => stamp,
        Err(error) => {
            return PriorStamp::Malformed(format!(
                "malformed deploy stamp: {error}"
            ));
        }
    };
    if stamp.version != STAMP_VERSION {
        return PriorStamp::Unsupported(stamp.version);
    }
    if stamp.digest != compute_digest(&stamp.assets) {
        return PriorStamp::Malformed(
            "deploy stamp digest does not match recorded assets".to_string(),
        );
    }
    let matches_vault = stamp_matches_vault(&stamp, vault);
    match stamp.state {
        StampState::Complete => PriorStamp::Complete {
            stamp,
            matches_vault,
        },
        StampState::Pending => PriorStamp::Pending {
            stamp,
            matches_vault,
        },
    }
}

pub(super) fn evaluate_guard(
    prior: &PriorStamp,
    captured: &CapturedPlugin,
    vault: &VaultBundle,
    current_host: &str,
    force: bool,
    incoming_repo: &Path,
    plugin_id: &str,
) -> GuardDecision {
    if force {
        return GuardDecision::Permit {
            stamp_action: StampAction::Write,
        };
    }

    let exact = exact_bundle(captured, vault);
    let incoming_clean = captured.meta.kind == SourceGitKind::Worktree
        && captured.meta.dirty == DirtyState::Clean
        && captured.meta.head.is_some();

    match prior {
        PriorStamp::Missing => GuardDecision::Permit {
            stamp_action: StampAction::Write,
        },
        PriorStamp::Complete { stamp, .. } if exact => {
            if incoming_clean
                && (stamp.dirty != DirtyState::Clean
                    || stamp.source_head.as_deref()
                        != captured.meta.head.as_deref()
                    || stamp_root(stamp) != captured.meta.root)
            {
                GuardDecision::Permit {
                    stamp_action: StampAction::Write,
                }
            } else {
                GuardDecision::Permit {
                    stamp_action: StampAction::Leave,
                }
            }
        }
        PriorStamp::Complete {
            stamp,
            matches_vault: true,
        } if stamp.dirty == DirtyState::Dirty => {
            let same_checkout = stamp_root(stamp) == captured.meta.root;
            let same_host = stamp.host == current_host;
            if same_checkout && same_host {
                match ancestor_of(incoming_repo, stamp.source_head.as_deref()) {
                    Ancestor::Yes => GuardDecision::Permit {
                        stamp_action: StampAction::Write,
                    },
                    _ => GuardDecision::Refuse {
                        message: refuse_message(
                            plugin_id,
                            stamp,
                            "same-checkout uncommitted deploy has a reset, \
divergent, or unknown history",
                            incoming_repo,
                        ),
                    },
                }
            } else {
                GuardDecision::Refuse {
                    message: refuse_message(
                        plugin_id,
                        stamp,
                        "HEAD ancestry cannot prove inclusion of uncommitted \
changes from another checkout or host",
                        incoming_repo,
                    ),
                }
            }
        }
        PriorStamp::Complete {
            stamp,
            matches_vault: true,
        } if stamp.dirty == DirtyState::Clean => {
            match ancestor_of(incoming_repo, stamp.source_head.as_deref()) {
                Ancestor::Yes => GuardDecision::Permit {
                    stamp_action: StampAction::Write,
                },
                _ => GuardDecision::Refuse {
                    message: refuse_message(
                        plugin_id,
                        stamp,
                        "incoming HEAD does not contain the stamped clean \
revision",
                        incoming_repo,
                    ),
                },
            }
        }
        PriorStamp::Complete { stamp, .. }
        | PriorStamp::Pending { stamp, .. } => {
            if exact && incoming_clean {
                GuardDecision::Permit {
                    stamp_action: StampAction::Write,
                }
            } else if exact {
                GuardDecision::Permit {
                    stamp_action: StampAction::Leave,
                }
            } else {
                let cause = match prior {
                    PriorStamp::Pending { .. } => {
                        "prior deploy is incomplete (pending stamp)"
                    }
                    PriorStamp::Complete {
                        matches_vault: false,
                        ..
                    } => "deploy stamp digest does not match vault bytes",
                    _ => "prior deploy provenance is unknown or unverified",
                };
                GuardDecision::Refuse {
                    message: refuse_message(
                        plugin_id,
                        stamp,
                        cause,
                        incoming_repo,
                    ),
                }
            }
        }
        PriorStamp::Malformed(detail) => {
            if exact && incoming_clean {
                GuardDecision::Permit {
                    stamp_action: StampAction::Write,
                }
            } else if exact {
                GuardDecision::Permit {
                    stamp_action: StampAction::Leave,
                }
            } else {
                GuardDecision::Refuse {
                    message: format!(
                        "refusing to overwrite plugin {plugin_id}: {detail}. \
Preview with `bob plugins sync --repo {} --no-pull --dry-run -p {plugin_id}`; \
overwrite with `bob plugins sync --repo {} -p {plugin_id} -F`.",
                        incoming_repo.display(),
                        incoming_repo.display()
                    ),
                }
            }
        }
        PriorStamp::Unsupported(version) => {
            if exact && incoming_clean {
                GuardDecision::Permit {
                    stamp_action: StampAction::Write,
                }
            } else if exact {
                GuardDecision::Permit {
                    stamp_action: StampAction::Leave,
                }
            } else {
                GuardDecision::Refuse {
                    message: format!(
                        "refusing to overwrite plugin {plugin_id}: unsupported \
deploy stamp version {version}. Preview with `bob plugins sync --repo {} \
--no-pull --dry-run -p {plugin_id}`; overwrite with `bob plugins sync --repo \
{} -p {plugin_id} -F`.",
                        incoming_repo.display(),
                        incoming_repo.display()
                    ),
                }
            }
        }
    }
}

fn stamp_root(stamp: &DeployStamp) -> PathBuf {
    canonical_path(Path::new(&stamp.source_root))
}

fn ancestor_of(repo: &Path, stamped_head: Option<&str>) -> Ancestor {
    let Some(stamped_head) = stamped_head.filter(|head| !head.is_empty())
    else {
        return Ancestor::Unknown;
    };
    let incoming_head = match git_head(repo) {
        Some(head) => head,
        None => return Ancestor::Unknown,
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(stamped_head)
        .arg(&incoming_head)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() => Ancestor::Yes,
        Ok(output) if output.status.code() == Some(1) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            if stderr.contains("not a valid object")
                || stderr.contains("bad object")
                || stderr.contains("unknown revision")
            {
                Ancestor::Unknown
            } else {
                Ancestor::No
            }
        }
        _ => Ancestor::Unknown,
    }
}

pub(super) fn refuse_message(
    plugin_id: &str,
    stamp: &DeployStamp,
    cause: &str,
    incoming_repo: &Path,
) -> String {
    let phrase = if stamp.dirty == DirtyState::Dirty {
        "deployed from uncommitted build"
    } else {
        "deployed"
    };
    let sha = stamp
        .source_head
        .as_deref()
        .map(short_sha)
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "refusing to overwrite plugin {plugin_id} {phrase} at {} on {} \
({sha}, {}). {cause}. Preview with `bob plugins sync --repo {} --no-pull \
--dry-run -p {plugin_id}`; overwrite with `bob plugins sync --repo {} \
-p {plugin_id} -F`.",
        stamp.source_root,
        stamp.host,
        stamp.deployed_at,
        incoming_repo.display(),
        incoming_repo.display()
    )
}

pub(super) fn short_sha(head: &str) -> String {
    head.chars().take(7).collect()
}

fn exact_bundle(captured: &CapturedPlugin, vault: &VaultBundle) -> bool {
    captured.assets.iter().all(|asset| match &asset.bytes {
        Some(bytes) => vault.files.get(asset.name).is_some_and(|v| v == bytes),
        None => !vault.files.contains_key(asset.name),
    })
}

fn stamp_matches_vault(stamp: &DeployStamp, vault: &VaultBundle) -> bool {
    stamp.assets.iter().all(|asset| {
        if !asset.present {
            return true;
        }
        match (asset.sha256.as_deref(), vault.files.get(&asset.name)) {
            (Some(expected), Some(bytes)) => sha256_hex(bytes) == expected,
            _ => false,
        }
    })
}

pub(super) fn fingerprints_from_captured(
    captured: &CapturedPlugin,
) -> Vec<AssetFingerprint> {
    captured
        .assets
        .iter()
        .map(|asset| match &asset.bytes {
            Some(bytes) => AssetFingerprint {
                name: asset.name.to_string(),
                present: true,
                sha256: Some(sha256_hex(bytes)),
            },
            None => AssetFingerprint {
                name: asset.name.to_string(),
                present: false,
                sha256: None,
            },
        })
        .collect()
}

pub(super) fn compute_digest(assets: &[AssetFingerprint]) -> String {
    let mut hasher = Sha256::new();
    for asset in assets {
        hasher.update(asset.name.as_bytes());
        hasher.update([0]);
        hasher.update([u8::from(asset.present)]);
        hasher.update([0]);
        if let Some(sha) = &asset.sha256 {
            hasher.update(sha.as_bytes());
        }
        hasher.update([b'\n']);
    }
    hex::encode(hasher.finalize())
}

pub(super) fn build_stamp(
    plugin_id: &str,
    captured: &CapturedPlugin,
    state: StampState,
    host: &str,
    deployed_at: &str,
    agent: Option<String>,
) -> DeployStamp {
    let assets = fingerprints_from_captured(captured);
    let digest = compute_digest(&assets);
    DeployStamp {
        version: STAMP_VERSION,
        plugin_id: plugin_id.to_string(),
        source_root: captured.meta.root.display().to_string(),
        source_head: captured.meta.head.clone(),
        dirty: captured.meta.dirty,
        host: host.to_string(),
        deployed_at: deployed_at.to_string(),
        agent,
        assets,
        digest,
        state,
    }
}

pub(super) fn write_stamp_atomic(
    path: &Path,
    stamp: &DeployStamp,
) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!("deploy stamp {} has no parent directory", path.display())
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create plugin directory {}: {error}",
            parent.display()
        )
    })?;
    let tmp =
        parent.join(format!(".bob-deploy.json.tmp.{}", std::process::id()));
    let body = serde_json::to_vec_pretty(stamp).map_err(|error| {
        format!("failed to serialize deploy stamp: {error}")
    })?;
    fs::write(&tmp, body).map_err(|error| {
        format!("failed to write deploy stamp {}: {error}", tmp.display())
    })?;
    fs::rename(&tmp, path).map_err(|error| {
        let _ = fs::remove_file(&tmp);
        format!("failed to publish deploy stamp {}: {error}", path.display())
    })
}

pub(super) fn backup_prior_stamp(
    stamp_path: &Path,
    backup_path: &Path,
) -> Result<(), String> {
    if !stamp_path.is_file() {
        return Ok(());
    }
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create stamp backup directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::copy(stamp_path, backup_path)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to back up deploy stamp to {}: {error}",
                backup_path.display()
            )
        })
}

pub(super) fn verify_copied_bytes(
    vault_plugin_dir: &Path,
    captured: &CapturedPlugin,
) -> Result<(), String> {
    for asset in &captured.assets {
        let Some(bytes) = &asset.bytes else {
            continue;
        };
        let actual =
            fs::read(vault_plugin_dir.join(asset.name)).map_err(|error| {
                format!(
                    "failed to verify {}: {error}",
                    vault_plugin_dir.join(asset.name).display()
                )
            })?;
        if &actual != bytes {
            return Err(format!(
                "{} did not match captured source bytes after copy",
                asset.name
            ));
        }
    }
    Ok(())
}

pub(super) fn deployment_info(stamp: &DeployStamp) -> DeploymentInfo {
    let status = match (stamp.state, stamp.dirty) {
        (StampState::Pending, _) => DeployStatus::Incomplete,
        (StampState::Complete, DirtyState::Dirty) => DeployStatus::Uncommitted,
        (StampState::Complete, DirtyState::Clean) => DeployStatus::Clean,
        (StampState::Complete, DirtyState::Unknown) => DeployStatus::Unknown,
    };
    DeploymentInfo {
        status,
        source_root: Some(stamp.source_root.clone()),
        source_head: stamp.source_head.clone(),
        host: Some(stamp.host.clone()),
        deployed_at: Some(stamp.deployed_at.clone()),
    }
}

pub(super) fn deployment_info_from_prior(
    prior: &PriorStamp,
) -> Option<DeploymentInfo> {
    match prior {
        PriorStamp::Missing => None,
        PriorStamp::Complete {
            stamp,
            matches_vault,
        } => {
            let mut info = deployment_info(stamp);
            if !*matches_vault {
                info.status = DeployStatus::Mismatched;
            }
            Some(info)
        }
        PriorStamp::Pending { stamp, .. } => {
            let mut info = deployment_info(stamp);
            info.status = DeployStatus::Incomplete;
            Some(info)
        }
        PriorStamp::Malformed(_) | PriorStamp::Unsupported(_) => {
            Some(DeploymentInfo {
                status: DeployStatus::Unknown,
                source_root: None,
                source_head: None,
                host: None,
                deployed_at: None,
            })
        }
    }
}

pub(super) fn scan_deployment(
    vault_plugin_dir: &Path,
) -> Option<DeploymentInfo> {
    let path = vault_plugin_dir.join(STAMP_FILE);
    if !path.is_file() {
        return None;
    }
    let vault = snapshot_vault(vault_plugin_dir);
    deployment_info_from_prior(&read_prior_stamp(&path, &vault))
}

pub(super) fn uncommitted_detail(info: &DeploymentInfo) -> Option<String> {
    match info.status {
        DeployStatus::Uncommitted => {
            let sha = info
                .source_head
                .as_deref()
                .map(short_sha)
                .unwrap_or_else(|| "unknown".to_string());
            Some(format!(
                "deployed from uncommitted build  {}  {}  {sha}  {}",
                info.source_root.as_deref().unwrap_or("unknown"),
                info.host.as_deref().unwrap_or("unknown"),
                info.deployed_at.as_deref().unwrap_or("unknown"),
            ))
        }
        DeployStatus::Unknown => Some("deploy provenance unknown".to_string()),
        DeployStatus::Incomplete => {
            Some("deploy provenance incomplete".to_string())
        }
        DeployStatus::Mismatched => {
            Some("deploy stamp does not match vault bytes".to_string())
        }
        DeployStatus::Clean => None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn one_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git command failed")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, present: bool, sha: Option<&str>) -> AssetFingerprint {
        AssetFingerprint {
            name: name.to_string(),
            present,
            sha256: sha.map(str::to_string),
        }
    }

    #[test]
    fn digest_is_stable_for_the_same_assets() {
        let assets = vec![
            asset("manifest.json", true, Some("aa")),
            asset("main.js", true, Some("bb")),
            asset("styles.css", false, None),
        ];
        assert_eq!(compute_digest(&assets), compute_digest(&assets));
        let mut shuffled = assets.clone();
        shuffled.swap(0, 1);
        assert_ne!(compute_digest(&assets), compute_digest(&shuffled));
    }

    #[test]
    fn leftover_css_does_not_break_stamp_match() {
        let stamp = DeployStamp {
            version: 1,
            plugin_id: "alpha".into(),
            source_root: "/src".into(),
            source_head: Some("abc".into()),
            dirty: DirtyState::Clean,
            host: "host".into(),
            deployed_at: "t".into(),
            agent: None,
            assets: vec![
                asset("manifest.json", true, Some(&sha256_hex(b"m"))),
                asset("main.js", true, Some(&sha256_hex(b"j"))),
                asset("styles.css", false, None),
            ],
            digest: String::new(),
            state: StampState::Complete,
        };
        let mut vault = VaultBundle {
            files: BTreeMap::new(),
        };
        vault.files.insert("manifest.json".into(), b"m".to_vec());
        vault.files.insert("main.js".into(), b"j".to_vec());
        vault.files.insert("styles.css".into(), b"css".to_vec());
        assert!(stamp_matches_vault(&stamp, &vault));
    }

    #[test]
    fn leftover_css_blocks_exact_bundle() {
        let captured = CapturedPlugin {
            meta: SourceMeta {
                root: PathBuf::from("/src"),
                head: Some("abc".into()),
                dirty: DirtyState::Clean,
                kind: SourceGitKind::Worktree,
            },
            assets: vec![
                CapturedAsset {
                    name: "manifest.json",
                    bytes: Some(b"m".to_vec()),
                },
                CapturedAsset {
                    name: "main.js",
                    bytes: Some(b"j".to_vec()),
                },
                CapturedAsset {
                    name: "styles.css",
                    bytes: None,
                },
            ],
        };
        let mut vault = VaultBundle {
            files: BTreeMap::new(),
        };
        vault.files.insert("manifest.json".into(), b"m".to_vec());
        vault.files.insert("main.js".into(), b"j".to_vec());
        vault.files.insert("styles.css".into(), b"css".to_vec());
        assert!(!exact_bundle(&captured, &vault));
        vault.files.remove("styles.css");
        assert!(exact_bundle(&captured, &vault));
    }

    #[test]
    fn missing_stamp_bootstraps() {
        let captured = CapturedPlugin {
            meta: SourceMeta {
                root: PathBuf::from("/src"),
                head: Some("abc".into()),
                dirty: DirtyState::Dirty,
                kind: SourceGitKind::Worktree,
            },
            assets: vec![CapturedAsset {
                name: "main.js",
                bytes: Some(b"j".to_vec()),
            }],
        };
        let vault = VaultBundle {
            files: BTreeMap::new(),
        };
        let decision = evaluate_guard(
            &PriorStamp::Missing,
            &captured,
            &vault,
            "host",
            false,
            Path::new("/src"),
            "alpha",
        );
        assert_eq!(
            decision,
            GuardDecision::Permit {
                stamp_action: StampAction::Write
            }
        );
    }

    #[test]
    fn force_permits_uncertain_stamps() {
        let captured = CapturedPlugin {
            meta: SourceMeta {
                root: PathBuf::from("/other"),
                head: Some("def".into()),
                dirty: DirtyState::Clean,
                kind: SourceGitKind::Worktree,
            },
            assets: vec![CapturedAsset {
                name: "main.js",
                bytes: Some(b"new".to_vec()),
            }],
        };
        let vault = VaultBundle {
            files: BTreeMap::from([("main.js".into(), b"old".to_vec())]),
        };
        let decision = evaluate_guard(
            &PriorStamp::Malformed("bad".into()),
            &captured,
            &vault,
            "host",
            true,
            Path::new("/other"),
            "alpha",
        );
        assert_eq!(
            decision,
            GuardDecision::Permit {
                stamp_action: StampAction::Write
            }
        );
    }
}
