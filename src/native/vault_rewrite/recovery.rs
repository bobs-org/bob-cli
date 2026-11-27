//! Durable intent journal for vault-rewrite transactions.
//!
//! A vault rewrite persists an intent under
//! `bob_cli_state_dir()/vault-rewrite/<vault-hash>/<run-id>/` (never in
//! the vault) before mutating the vault, so a crash mid-write cannot
//! leave half a rewrite that the next online sync would commit. The next
//! `vault_rewrite::run` recovers under the same lock before planning, and
//! the vault-sync pre-cycle recovers the same way. Recovery then:
//!
//! * the recorded rewrite commit is present → clear the intent;
//! * every file still holds its before- or after-image → roll back to
//!   the before-images, remove created files, move trashed files back,
//!   and clear the intent;
//! * anything else → refuse with `recovery_pending`, naming the manifest
//!   and every path holding neither image. Never commit half a rewrite.
//!
//! Before-image copies live beside `intent.json` as `<index>.before` so
//! recovery never trusts the planner's memory, only the journal. Trash
//! destinations are pre-assigned (Obsidian ` 2` suffixes) and recorded in
//! the intent before the first write.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::TxnFailure;
use crate::native::{env as bob_env, ob};

/// Machine-readable failure code for a vault rewrite blocked by an
/// interrupted transaction that recovery could not roll back.
pub(crate) const RECOVERY_PENDING_CODE: &str = "recovery_pending";

/// Intent schema version; bump when the JSON shape changes.
const SCHEMA_VERSION: u32 = 1;

/// File holding the pending intent inside one run directory.
const INTENT_FILE: &str = "intent.json";

/// What one journaled path does: rewrite in place, create, or move to
/// the vault trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JournalAction {
    Rewrite,
    Create,
    Trash,
}

/// One journaled file: vault-relative path, action, before/after SHA-256
/// (`None` means absence: before for creates, after for trash), and the
/// vault-relative trash destination for trash actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JournalFile {
    pub(crate) path: String,
    pub(crate) action: JournalAction,
    pub(crate) before_sha256: Option<String>,
    pub(crate) after_sha256: Option<String>,
    #[serde(default)]
    pub(crate) trash_destination: Option<String>,
}

/// Durable record of one in-flight vault rewrite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VaultRewriteIntent {
    schema_version: u32,
    /// Canonical vault path string the intent was written for.
    pub(crate) vault: String,
    pub(crate) vault_hash: String,
    pub(crate) run_id: String,
    /// Expected transaction commit subject.
    pub(crate) subject: String,
    /// Clean HEAD before the first write; `None` on an unborn HEAD.
    pub(crate) base_head: Option<String>,
    /// Commit SHA the transaction created, recorded after the commit.
    pub(crate) expected_head: Option<String>,
    pub(crate) files: Vec<JournalFile>,
}

/// One file handed to the journal: vault-relative path, action, and the
/// live before/after bytes (`None` before for creates, `None` after for
/// trash).
pub(crate) struct PendingJournalFile<'a> {
    pub(crate) path: &'a str,
    pub(crate) action: JournalAction,
    pub(crate) before_bytes: Option<&'a [u8]>,
    pub(crate) after_bytes: Option<&'a [u8]>,
}

/// A persisted intent: its run directory plus the journaled files with
/// their assigned trash destinations.
pub(crate) struct PersistedIntent {
    pub(crate) run_dir: PathBuf,
    pub(crate) files: Vec<JournalFile>,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn new_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{now:x}-{:x}-{:x}",
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn vault_hash(canonical_vault: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        sha256_hex(canonical_vault.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        sha256_hex(canonical_vault.to_string_lossy().as_bytes())
    }
}

fn intent_root() -> PathBuf {
    bob_env::bob_cli_state_dir().join("vault-rewrite")
}

fn vault_intent_dir(canonical_vault: &Path) -> PathBuf {
    intent_root().join(vault_hash(canonical_vault))
}

fn canonical_vault_string(vault: &Path) -> Result<String, TxnFailure> {
    fs::canonicalize(vault)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            TxnFailure::new(
                "io",
                format!("could not resolve the vault path: {error}"),
                Some("check that the Bob vault exists and retry"),
            )
        })
}

/// Reject absolute paths, parent escapes, and `.git/` targets before an
/// intent (or its recovery) touches the vault.
fn validated_relative(path: &str) -> Result<PathBuf, String> {
    let relative = PathBuf::from(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(component, Component::ParentDir | Component::Prefix(_))
        })
    {
        return Err(format!("intent path escapes the vault: {path}"));
    }
    let text = path.replace('\\', "/");
    if text == ".git"
        || text.starts_with(".git/")
        || text.starts_with("./.git/")
    {
        return Err(format!("intent path targets .git: {path}"));
    }
    Ok(relative)
}

fn sync_file(file: &fs::File, what: &Path) -> Result<(), TxnFailure> {
    file.sync_all().map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not fsync {}: {error}", what.display()),
            Some("check disk space and retry"),
        )
    })
}

fn sync_dir(dir: &Path) -> Result<(), TxnFailure> {
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Pick the trash destination for `file_name`: `.trash/<name>`, using
/// Obsidian's ` 2`, ` 3` suffixes on collision. Creates `.trash/`.
pub(crate) fn pick_trash_destination(
    vault: &Path,
    file_name: &str,
) -> Result<(PathBuf, String), TxnFailure> {
    let trash = vault.join(".trash");
    fs::create_dir_all(&trash).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not create {}: {error}", trash.display()),
            Some("check vault permissions and retry"),
        )
    })?;
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty() && !extension.is_empty() =>
        {
            (stem.to_string(), Some(extension.to_string()))
        }
        _ => (file_name.to_string(), None),
    };
    for attempt in 0.. {
        let candidate_name = if attempt == 0 {
            file_name.to_string()
        } else {
            match &extension {
                Some(extension) => {
                    format!(
                        "{} {}{}",
                        stem,
                        attempt + 1,
                        format!(".{extension}")
                    )
                }
                None => format!("{stem} {}", attempt + 1),
            }
        };
        let candidate = trash.join(&candidate_name);
        if !candidate.exists() {
            let relative = format!(".trash/{candidate_name}");
            return Ok((candidate, relative));
        }
    }
    unreachable!("trash suffix search always terminates")
}

fn write_intent_file(
    run_dir: &Path,
    intent: &VaultRewriteIntent,
) -> Result<(), TxnFailure> {
    let text = serde_json::to_string(intent).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not serialize the vault-rewrite intent: {error}"),
            Some("check disk space and retry"),
        )
    })?;
    let temporary = run_dir.join(format!("{INTENT_FILE}.tmp"));
    fs::write(&temporary, text.as_bytes()).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not write {}: {error}", temporary.display()),
            Some("check disk space and retry"),
        )
    })?;
    let handle = fs::File::open(&temporary).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not fsync {}: {error}", temporary.display()),
            Some("check disk space and retry"),
        )
    })?;
    sync_file(&handle, &temporary)?;
    drop(handle);
    fs::rename(&temporary, run_dir.join(INTENT_FILE)).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not install the vault-rewrite intent: {error}"),
            Some("check disk space and retry"),
        )
    })?;
    sync_dir(run_dir)
}

/// Persist the intent before the first write: before-image copies first,
/// then `intent.json`, everything fsynced. Trash destinations are
/// pre-assigned so recovery can find trashed files after a crash.
pub(crate) fn persist_intent(
    vault: &Path,
    subject: &str,
    base_head: Option<&str>,
    files: &[PendingJournalFile<'_>],
) -> Result<PersistedIntent, TxnFailure> {
    let canonical = canonical_vault_string(vault)?;
    let canonical_path = PathBuf::from(&canonical);
    let hash = vault_hash(&canonical_path);
    let run_id = new_run_id();
    let run_dir = intent_root().join(&hash).join(&run_id);
    fs::create_dir_all(&run_dir).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not create {}: {error}", run_dir.display()),
            Some("check disk space and retry"),
        )
    })?;
    let mut journaled = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        validated_relative(file.path).map_err(|message| {
            TxnFailure::new(
                "verify_failed",
                format!("cannot journal {message}"),
                Some("retry the rewrite without that path"),
            )
        })?;
        if file.action == JournalAction::Trash && file.before_bytes.is_none() {
            return Err(TxnFailure::new(
                "verify_failed",
                format!("cannot trash {} without a before-image", file.path),
                Some("replan the rewrite and retry"),
            ));
        }
        if let Some(before) = file.before_bytes
            && matches!(
                file.action,
                JournalAction::Rewrite | JournalAction::Trash
            )
        {
            let copy = run_dir.join(format!("{index:04}.before"));
            fs::write(&copy, before).map_err(|error| {
                TxnFailure::new(
                    "verify_failed",
                    format!("could not write {}: {error}", copy.display()),
                    Some("check disk space and retry"),
                )
            })?;
            let handle = fs::File::open(&copy).map_err(|error| {
                TxnFailure::new(
                    "verify_failed",
                    format!("could not fsync {}: {error}", copy.display()),
                    Some("check disk space and retry"),
                )
            })?;
            sync_file(&handle, &copy)?;
        }
        let trash_destination = if file.action == JournalAction::Trash {
            let file_name = Path::new(file.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("note.md");
            Some(pick_trash_destination(vault, file_name)?.1)
        } else {
            None
        };
        journaled.push(JournalFile {
            path: file.path.to_string(),
            action: file.action,
            before_sha256: file.before_bytes.map(sha256_hex),
            after_sha256: file.after_bytes.map(sha256_hex),
            trash_destination,
        });
    }
    let intent = VaultRewriteIntent {
        schema_version: SCHEMA_VERSION,
        vault: canonical,
        vault_hash: hash,
        run_id,
        subject: subject.to_string(),
        base_head: base_head.map(str::to_string),
        expected_head: None,
        files: journaled.clone(),
    };
    write_intent_file(&run_dir, &intent)?;
    Ok(PersistedIntent {
        run_dir,
        files: journaled,
    })
}

/// Record the commit SHA in the intent after the commit, before removing
/// the intent. A crash in between still proves the commit.
pub(crate) fn record_commit(
    run_dir: &Path,
    sha: &str,
) -> Result<(), TxnFailure> {
    let mut intent = read_intent(run_dir).map_err(|message| {
        TxnFailure::new(
            "verify_failed",
            format!("rewrite committed but {message}"),
            Some("the commit is safe; remove the manifest and retry"),
        )
    })?;
    intent.expected_head = Some(sha.to_string());
    write_intent_file(run_dir, &intent)
}

/// Update one trash destination after an install-time collision bump.
pub(crate) fn record_trash_destination(
    run_dir: &Path,
    index: usize,
    destination: &str,
) -> Result<(), TxnFailure> {
    let mut intent = read_intent(run_dir).map_err(|message| {
        TxnFailure::new(
            "verify_failed",
            format!("trashed a file but {message}"),
            Some("the trash copy is safe; remove the manifest and retry"),
        )
    })?;
    let Some(entry) = intent.files.get_mut(index) else {
        return Err(TxnFailure::new(
            "verify_failed",
            "trashed a file but the intent has no matching entry".to_string(),
            Some("the trash copy is safe; remove the manifest and retry"),
        ));
    };
    entry.trash_destination = Some(destination.to_string());
    write_intent_file(run_dir, &intent)
}

/// Remove a completed or rolled-back intent.
pub(crate) fn clear_intent(run_dir: &Path) {
    let _ = fs::remove_dir_all(run_dir);
}

fn read_intent(run_dir: &Path) -> Result<VaultRewriteIntent, String> {
    let path = run_dir.join(INTENT_FILE);
    let text = fs::read_to_string(&path).map_err(|error| {
        format!("could not read {}: {error}", path.display())
    })?;
    serde_json::from_str::<VaultRewriteIntent>(&text).map_err(|error| {
        format!("ignoring malformed intent at {} ({error})", path.display())
    })
}

fn recovery_hint() -> String {
    "run bob vault-sync on this host to recover the interrupted rewrite"
        .to_string()
}

fn pending_error(manifest: &Path, detail: impl Into<String>) -> TxnFailure {
    TxnFailure::new(
        RECOVERY_PENDING_CODE,
        format!(
            "interrupted vault rewrite is pending (manifest {}); {}; {}",
            manifest.display(),
            detail.into(),
            recovery_hint(),
        ),
        Some(recovery_hint()),
    )
}

/// Recover every pending intent for this vault. Runs under the caller's
/// lock: `vault_rewrite::run` before planning, and the vault-sync
/// pre-cycle. Returns one warning when intents rolled back.
pub(crate) fn recover_pending(
    vault: &Path,
    child_env: &ob::ChildEnv,
) -> Result<Option<String>, TxnFailure> {
    let canonical = match fs::canonicalize(vault) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(TxnFailure::new(
                "io",
                format!("could not resolve the vault path: {error}"),
                Some("check that the Bob vault exists and retry"),
            ));
        }
    };
    let dir = vault_intent_dir(&canonical);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(TxnFailure::new(
                "io",
                format!("could not read {}: {error}", dir.display()),
                Some("check the state directory and retry"),
            ));
        }
    };
    let mut run_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join(INTENT_FILE).exists())
        .collect();
    run_dirs.sort();
    let mut notices = Vec::new();
    for run_dir in run_dirs {
        if let Some(notice) =
            recover_one(vault, child_env, &canonical, &run_dir)?
        {
            notices.push(notice);
        }
    }
    Ok(notices.into_iter().next())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    Before,
    After,
    Neither,
}

fn live_state(
    vault: &Path,
    file: &JournalFile,
) -> Result<FileState, TxnFailure> {
    let absolute = vault.join(&file.path);
    let live = fs::read(&absolute).ok();
    let live_sha = live.as_deref().map(sha256_hex);
    match file.action {
        JournalAction::Rewrite => {
            if live_sha == file.before_sha256 {
                Ok(FileState::Before)
            } else if live_sha == file.after_sha256 {
                Ok(FileState::After)
            } else {
                Ok(FileState::Neither)
            }
        }
        JournalAction::Create => {
            if live.is_none() {
                Ok(FileState::Before)
            } else if live_sha == file.after_sha256 {
                Ok(FileState::After)
            } else {
                Ok(FileState::Neither)
            }
        }
        JournalAction::Trash => {
            if live_sha == file.before_sha256 {
                return Ok(FileState::Before);
            }
            if live.is_some() {
                return Ok(FileState::Neither);
            }
            let Some(destination) = file.trash_destination.as_deref() else {
                return Ok(FileState::Neither);
            };
            let dest_bytes = fs::read(vault.join(destination)).ok();
            if dest_bytes.as_deref().map(sha256_hex) == file.before_sha256 {
                Ok(FileState::After)
            } else {
                Ok(FileState::Neither)
            }
        }
    }
}

fn commit_subject(
    vault: &Path,
    child_env: &ob::ChildEnv,
    sha: &str,
) -> Option<String> {
    let output = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%s", sha])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_ancestor(
    vault: &Path,
    child_env: &ob::ChildEnv,
    ancestor: &str,
    head: &str,
) -> bool {
    ob::git_command(vault, child_env)
        .args(["merge-base", "--is-ancestor", ancestor, head])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// True when the recorded commit SHA exists with the expected subject
/// and is an ancestor of HEAD. A generic matching subject alone never
/// proves it.
fn commit_proves(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &VaultRewriteIntent,
    head: &str,
) -> bool {
    let Some(expected) = intent.expected_head.as_deref() else {
        return false;
    };
    commit_subject(vault, child_env, expected)
        .is_some_and(|subject| subject == intent.subject)
        && is_ancestor(vault, child_env, expected, head)
}

/// True when HEAD itself carries the expected subject and its tree holds
/// every after-image. Covers a crash between the commit and recording
/// the SHA.
fn head_tree_proves(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &VaultRewriteIntent,
    head: &str,
) -> bool {
    if commit_subject(vault, child_env, head)
        .is_none_or(|subject| subject != intent.subject)
    {
        return false;
    }
    intent.files.iter().all(|file| {
        // `git show <head>:<path>` prints the committed bytes; trash
        // paths must be absent from the tree.
        let committed = ob::git_command(vault, child_env)
            .args(["show", &format!("{head}:{}", file.path)])
            .output();
        match committed {
            Ok(result) if result.status.success() => {
                file.action != JournalAction::Trash
                    && file
                        .after_sha256
                        .as_deref()
                        .is_some_and(|sha| sha256_hex(&result.stdout) == *sha)
            }
            _ => file.action == JournalAction::Trash,
        }
    })
}

fn recover_one(
    vault: &Path,
    child_env: &ob::ChildEnv,
    canonical: &Path,
    run_dir: &Path,
) -> Result<Option<String>, TxnFailure> {
    let manifest = run_dir.join(INTENT_FILE);
    let intent = read_intent(run_dir).map_err(|message| {
        pending_error(&manifest, format!("{message}; remove it to proceed"))
    })?;
    if intent.schema_version != SCHEMA_VERSION {
        return Err(pending_error(
            &manifest,
            format!(
                "unsupported intent schema {}; remove it to proceed",
                intent.schema_version
            ),
        ));
    }
    if Path::new(&intent.vault) != canonical {
        return Err(pending_error(
            &manifest,
            format!(
                "ignoring intent for another vault ({}); remove it to proceed",
                intent.vault
            ),
        ));
    }
    for file in &intent.files {
        if let Err(message) = validated_relative(&file.path) {
            return Err(pending_error(&manifest, message));
        }
        if let Some(destination) = file.trash_destination.as_deref()
            && let Err(message) = validated_relative(destination)
        {
            return Err(pending_error(&manifest, message));
        }
    }
    let head = super::git_head(vault, child_env).map_err(|detail| {
        pending_error(
            &manifest,
            format!("could not read the vault HEAD: {detail}"),
        )
    })?;
    if commit_proves(vault, child_env, &intent, &head)
        || head_tree_proves(vault, child_env, &intent, &head)
    {
        clear_intent(run_dir);
        return Ok(None);
    }
    if intent.base_head.as_deref() != Some(head.as_str()) {
        return Err(pending_error(
            &manifest,
            format!(
                "the vault moved since the rewrite began (HEAD {head} is not the recorded base {}); left the intent in place",
                intent.base_head.as_deref().unwrap_or("(unborn)"),
            ),
        ));
    }
    let mut states = BTreeMap::new();
    let mut neither = Vec::new();
    for file in &intent.files {
        let state = live_state(vault, file)?;
        if state == FileState::Neither {
            neither.push(file.path.clone());
        }
        states.insert(file.path.clone(), state);
    }
    if !neither.is_empty() {
        neither.sort();
        return Err(pending_error(
            &manifest,
            format!(
                "{} path(s) hold neither the before- nor the after-image: {}",
                neither.len(),
                neither.join(", "),
            ),
        ));
    }
    let mut failures = Vec::new();
    for (index, file) in intent.files.iter().enumerate() {
        let state = states[&file.path];
        if let Err(message) = rollback_file(vault, run_dir, index, file, state)
        {
            failures.push(format!("{}: {message}", file.path));
        }
    }
    if !failures.is_empty() {
        failures.sort();
        return Err(pending_error(
            &manifest,
            format!("rollback failed: {}", failures.join("; ")),
        ));
    }
    clear_intent(run_dir);
    Ok(Some(format!(
        "recovered: rolled back an interrupted vault rewrite '{}' ({} file(s))",
        intent.subject,
        intent.files.len(),
    )))
}

fn rollback_file(
    vault: &Path,
    run_dir: &Path,
    index: usize,
    file: &JournalFile,
    state: FileState,
) -> Result<(), String> {
    let absolute = vault.join(&file.path);
    match (file.action, state) {
        (_, FileState::Before) => Ok(()),
        (JournalAction::Rewrite, FileState::After) => {
            let copy = run_dir.join(format!("{index:04}.before"));
            let before = fs::read(&copy).map_err(|error| {
                format!("could not read {}: {error}", copy.display())
            })?;
            // Byte-exact restore (notes are UTF-8, but the journal
            // keeps bytes so recovery never re-encodes).
            fs::write(&absolute, &before)
                .map_err(|error| format!("could not restore: {error}"))?;
            fs::File::open(&absolute)
                .and_then(|handle| handle.sync_all())
                .map_err(|error| format!("could not fsync: {error}"))
        }
        (JournalAction::Create, FileState::After) => fs::remove_file(&absolute)
            .map_err(|error| format!("could not remove: {error}")),
        (JournalAction::Trash, FileState::After) => {
            let destination =
                file.trash_destination.as_deref().ok_or_else(|| {
                    "the trash destination was never recorded".to_string()
                })?;
            let absolute_dest = vault.join(destination);
            let before = file.before_sha256.as_deref().ok_or_else(|| {
                "the trash before-image is missing".to_string()
            })?;
            if absolute.exists() {
                return Err(
                    "a new file appeared at the trashed path".to_string()
                );
            }
            let current = fs::read(&absolute_dest).map_err(|error| {
                format!("could not read {}: {error}", absolute_dest.display())
            })?;
            if sha256_hex(&current) != *before {
                return Err(format!(
                    "the trash copy at {destination} no longer holds the before-image"
                ));
            }
            fs::rename(&absolute_dest, &absolute).map_err(|error| {
                format!("could not move back from {destination}: {error}")
            })
        }
        (_, FileState::Neither) => {
            Err("the file holds neither image".to_string())
        }
    }
}
