//! Durable overwrite intents and scoped HEAD recovery.
//!
//! An overwrite transaction persists an intent in
//! `bob_cli_state_dir()/ref/overwrite-intents/` (never in the vault) before
//! mutating the vault, so a crash cannot lose the earlier capture. The next
//! real `ref create` or writing `ref scan` (including intake-only and JSON
//! variants) recovers under the same lock order before any pre-sync can
//! commit half-applied files:
//!
//! * the recorded overwrite commit is present → keep the new state and
//!   clear the intent;
//! * otherwise → restore the touched paths from the recorded clean HEAD,
//!   remove only recorded creations, unstage those paths, and clear the
//!   intent.
//!
//! A generic matching commit subject alone never proves the commit: proof
//! needs the recorded commit SHA to exist with the expected subject and to
//! be an ancestor of HEAD. Dry runs stay read-only and stop with a
//! recovery-required refusal. Malformed intents and vaults that moved since
//! the intent was written fail without broad restore/clean operations, and
//! the marker is preserved whenever repair fails.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{reference_mutation, CommandError, Config};
use crate::native::{env as bob_env, ob};

/// Intent schema version; bump when the JSON shape changes.
const INTENT_SCHEMA_VERSION: u32 = 1;

/// Directory (under the state root's `ref/`) holding one intent file per
/// canonical vault.
const INTENT_DIR_NAME: &str = "overwrite-intents";

/// Machine-readable failure code for a dry run blocked by a pending
/// intent. Human rendering ignores it; JSON scans surface it.
const RECOVERY_REQUIRED_CODE: &str = "recovery_required";

/// Durable record of one in-flight overwrite transaction.
///
/// `touched` lists vault-relative paths that existed at `base_head` and may
/// have been modified; `created` lists vault-relative paths the transaction
/// added (directory operations expanded to their actual file manifest, so
/// untracked contents are never deleted with a sidecar directory).
/// `expected_head` is the commit SHA the transaction created, recorded once
/// the commit is proved; it distinguishes this overwrite from a previous
/// same-stem overwrite carrying the same subject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OverwriteIntent {
    schema_version: u32,
    /// Canonical vault path string the intent was written for.
    pub(crate) vault: String,
    /// Reference stem being overwritten (`lib`/note stem without suffix).
    pub(crate) stem: String,
    /// Reference type directory (`papers`, `blogs`, …).
    pub(crate) ref_type: String,
    /// Clean locked HEAD after pre-sync: the rollback base.
    pub(crate) base_head: String,
    /// Expected transaction commit subject
    /// (`bob ref create: overwrite <type>/<stem>`).
    pub(crate) expected_subject: String,
    /// Commit SHA the transaction created, once proved; `None` while the
    /// transaction has not committed yet.
    pub(crate) expected_head: Option<String>,
    /// Vault-relative paths that existed at the base and may be modified.
    pub(crate) touched: Vec<String>,
    /// Vault-relative paths the transaction created.
    pub(crate) created: Vec<String>,
}

/// Persist (or replace) the intent for `config`'s vault atomically: write
/// a temp file, fsync it, rename over the destination, then fsync the
/// directory. Consumed by the transaction phase.
#[allow(dead_code)]
pub(crate) fn save_intent(
    config: &Config,
    intent: &OverwriteIntent,
) -> std::result::Result<PathBuf, String> {
    save_intent_to(&state_ref_dir(), config, intent)
}

/// Load the pending intent for `config`'s vault, if any. A present but
/// unreadable file is a hard error: recovery must fail closed rather than
/// treat a half-written marker as absent. Consumed by the transaction
/// phase.
#[allow(dead_code)]
pub(crate) fn load_intent(
    config: &Config,
) -> std::result::Result<Option<OverwriteIntent>, CommandError> {
    load_intent_from(&state_ref_dir(), config)
}

/// Remove the intent for `config`'s vault. Missing files are fine.
/// Consumed by the transaction phase alongside recovery's own clears.
#[allow(dead_code)]
pub(crate) fn clear_intent(config: &Config) -> std::result::Result<(), String> {
    let canonical =
        canonical_vault(config).map_err(|error| error.to_string())?;
    let path = intent_path_for(&state_ref_dir(), &canonical);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not clear overwrite intent {}: {error}",
            path.display()
        )),
    }
}

/// Entry hook for commands that do not hold locks yet (`ref create` and
/// intake-only scans): fast-path when no intent exists without taking any
/// lock, otherwise recover under a fresh mutation session and release it
/// before returning. Returns a stderr notice when recovery ran, `None`
/// when there was nothing to do.
pub(crate) fn recover_before_command(
    config: &Config,
    dry_run: bool,
) -> std::result::Result<Option<String>, CommandError> {
    let state_dir = state_ref_dir();
    let Some(intent) = load_intent_from(&state_dir, config)? else {
        return Ok(None);
    };
    if dry_run {
        return Err(recovery_required(&intent));
    }
    let child_env = ob::child_env();
    let vault = &config.bob_dir;
    let _locks = reference_mutation::acquire_mutation_session(
        vault,
        &child_env,
        || eprintln!("waiting for another vault maintenance run…"),
    )
    .map_err(|error| match error {
        reference_mutation::MutationSessionError::Maintenance(error) => {
            CommandError::new(format!(
                "could not acquire the vault maintenance lock: {error}"
            ))
            .with_code("lock_busy")
        }
        reference_mutation::MutationSessionError::NotAWorktree => {
            CommandError::new(format!(
                "could not recover the interrupted overwrite of {}: the vault is not a Git worktree",
                intent.stem
            ))
        }
        reference_mutation::MutationSessionError::Scan(error) => error,
    })?;
    recover_locked(&state_dir, config, &child_env, &intent, false)
}

/// Entry hook for full scans, called after they hold the maintenance and
/// writer locks: recover without relocking. Dry runs perform no mutation
/// and need no locks; they only report a pending intent.
pub(crate) fn recover_with_locks_held(
    config: &Config,
    child_env: &ob::ChildEnv,
    dry_run: bool,
) -> std::result::Result<Option<String>, CommandError> {
    let state_dir = state_ref_dir();
    let Some(intent) = load_intent_from(&state_dir, config)? else {
        return Ok(None);
    };
    recover_locked(&state_dir, config, child_env, &intent, dry_run)
}

/// Core recovery: validate, prove-or-restore, then clear. Never broadens
/// scope on failure: malformed intents, path escapes, moved vaults, and
/// failed repairs all preserve the marker.
fn recover_locked(
    state_dir: &Path,
    config: &Config,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    dry_run: bool,
) -> std::result::Result<Option<String>, CommandError> {
    let vault = &config.bob_dir;
    let canonical = canonical_vault(config)?;
    if intent.vault != canonical {
        return Err(CommandError::new(format!(
            "ignoring overwrite intent for another vault ({}); remove it to proceed",
            intent_display_path(state_dir, &canonical).display()
        )));
    }
    let touched = validated_paths(&intent.touched)?;
    let created = validated_paths(&intent.created)?;
    if dry_run {
        return Err(recovery_required(intent));
    }
    if !reference_mutation::vault_is_git_worktree(vault, child_env) {
        return Err(CommandError::new(format!(
            "could not recover the interrupted overwrite of {}: the vault is not a Git worktree",
            intent.stem
        )));
    }
    let head =
        reference_mutation::git_head(vault, child_env).map_err(|error| {
            preserve_message(
                intent,
                &format!("could not read the vault HEAD: {error}"),
            )
        })?;
    if overwrite_committed(vault, child_env, intent, &head) {
        clear_intent_in(state_dir, &canonical).map_err(|error| {
            preserve_message(
                intent,
                &format!("the overwrite finished but {error}"),
            )
        })?;
        return Ok(Some(format!(
            "recovered: kept the completed overwrite of {}",
            intent.stem
        )));
    }
    if head != intent.base_head {
        return Err(preserve_message(
            intent,
            &format!(
                "the vault moved since the overwrite began (HEAD {head} is not the recorded base {}); left the intent in place",
                intent.base_head
            ),
        ));
    }
    restore_from_base(vault, child_env, intent, &touched, &created)?;
    clear_intent_in(state_dir, &canonical).map_err(|error| {
        preserve_message(intent, &format!("rolled back but {error}"))
    })?;
    Ok(Some(format!(
        "recovered: rolled back an interrupted overwrite of {}",
        intent.stem
    )))
}

/// True when the recorded overwrite commit is present: its SHA exists with
/// the expected subject and is an ancestor of HEAD. A generic matching
/// subject alone (including a previous same-stem overwrite) never proves
/// it. When the transaction committed but crashed before recording the
/// SHA, the HEAD commit carrying the expected subject proves it instead.
fn overwrite_committed(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    head: &str,
) -> bool {
    if let Some(expected) = intent.expected_head.as_deref() {
        return commit_subject(vault, child_env, expected)
            .is_some_and(|subject| subject == intent.expected_subject)
            && is_ancestor(vault, child_env, expected, head);
    }
    head != intent.base_head
        && commit_subject(vault, child_env, head)
            .is_some_and(|subject| subject == intent.expected_subject)
}

/// Subject line of one commit, or `None` when it cannot be read.
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

/// True when `ancestor` is an ancestor of `head`.
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

/// Restore `touched` from the recorded base, remove `created`, and unstage
/// exactly those paths. Unrelated working-tree and index content is left
/// untouched; any failure preserves the intent for a retry.
fn restore_from_base(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    touched: &[PathBuf],
    created: &[PathBuf],
) -> std::result::Result<(), CommandError> {
    if !touched.is_empty() {
        let status = ob::git_command(vault, child_env)
            .arg("checkout")
            .arg(&intent.base_head)
            .arg("--")
            .args(touched)
            .status()
            .map_err(|error| {
                preserve_message(
                    intent,
                    &format!("could not restore vault files: {error}"),
                )
            })?;
        if !status.success() {
            return Err(preserve_message(
                intent,
                &format!(
                    "could not restore vault files from {}",
                    intent.base_head
                ),
            ));
        }
    }
    let mut removals = Vec::new();
    for path in created {
        if remove_created_path(
            vault,
            path,
            &BTreeSet::from_iter(created.iter().cloned()),
        )
        .is_err()
        {
            removals.push(reference_mutation::vault_path(path));
        }
    }
    if !removals.is_empty() {
        return Err(preserve_message(
            intent,
            &format!(
                "could not remove created path(s): {}",
                removals.join(", ")
            ),
        ));
    }
    let mut staged: Vec<PathBuf> = touched.to_vec();
    staged.extend(created.iter().cloned());
    if !staged.is_empty()
        && let Err(error) =
            reference_mutation::unstage_paths(vault, child_env, &staged)
    {
        return Err(preserve_message(
            intent,
            &format!("could not unstage recovered paths: {error}"),
        ));
    }
    if !touched.is_empty() {
        let differs = ob::git_command(vault, child_env)
            .args(["diff", "--quiet", &intent.base_head, "--"])
            .args(touched)
            .status()
            .map(|status| !status.success())
            .unwrap_or(true);
        if differs {
            return Err(preserve_message(
                intent,
                "restored files still differ from the recorded base",
            ));
        }
    }
    Ok(())
}

/// Remove one recorded creation: files and symlinks directly, directories
/// only by deleting listed files inside them (then empty parents), so
/// untracked contents the transaction never recorded survive. Missing
/// paths are already restored.
fn remove_created_path(
    vault: &Path,
    path: &Path,
    created: &BTreeSet<PathBuf>,
) -> std::io::Result<()> {
    let absolute = vault.join(path);
    let symlink_type = fs::symlink_metadata(&absolute);
    if symlink_type.is_err() {
        return Ok(());
    }
    // Symlinks (even ones pointing at directories) are removed directly
    // and never traversed. Newly emptied parents are pruned (only while
    // empty, so pre-existing contents always stop the climb).
    let is_real_dir = symlink_type
        .as_ref()
        .is_ok_and(|metadata| metadata.is_dir());
    if !is_real_dir {
        remove_file_or_link(&absolute)?;
        if let Some(parent) = absolute.parent() {
            remove_empty_parents(parent, vault);
        }
        return Ok(());
    }
    // A recorded directory: delete only recorded files beneath it, then
    // prune parents that are left empty.
    let mut stack = vec![absolute.clone()];
    let mut files = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let entry_path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                stack.push(entry_path);
            } else {
                files.push(entry_path);
            }
        }
    }
    for file in &files {
        let relative = file.strip_prefix(vault).unwrap_or(file).to_path_buf();
        if created.contains(&relative) {
            remove_file_or_link(file)?;
        }
    }
    remove_empty_parents(&absolute, vault);
    Ok(())
}

fn remove_file_or_link(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Remove `directory` and its ancestors up to (not including) `vault`
/// while they are empty.
fn remove_empty_parents(directory: &Path, vault: &Path) {
    let mut current = directory.to_path_buf();
    while current != vault
        && current.starts_with(vault)
        && fs::remove_dir(&current).is_ok()
    {
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
}

/// Reject vault-escaping or non-relative intent paths without touching the
/// vault. `.git` roots are rejected outright: recovery never restores or
/// cleans repository internals.
fn validated_paths(
    paths: &[String],
) -> std::result::Result<Vec<PathBuf>, CommandError> {
    let mut validated = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = Path::new(raw);
        let mut components = path.components().peekable();
        let relative = matches!(
            components.peek(),
            Some(Component::Normal(_)) | Some(Component::CurDir)
        ) && !path.is_absolute();
        let mut escaped = !relative;
        let mut under_git = false;
        for component in path.components() {
            match component {
                Component::ParentDir => escaped = true,
                Component::Normal(name)
                    if name.eq_ignore_ascii_case(".git") =>
                {
                    under_git = true;
                }
                _ => {}
            }
        }
        if escaped || under_git {
            return Err(CommandError::new(format!(
                "ignoring malformed overwrite intent with invalid path {raw}; remove it to proceed"
            )));
        }
        validated.push(path.to_path_buf());
    }
    Ok(validated)
}

fn recovery_required(intent: &OverwriteIntent) -> CommandError {
    CommandError::new(format!(
        "recovery required: an interrupted overwrite of {} is pending; run a writing command without --dry-run to recover",
        intent.stem
    ))
    .with_code(RECOVERY_REQUIRED_CODE)
}

/// Wrap a repair failure so the report states the marker was preserved.
fn preserve_message(intent: &OverwriteIntent, detail: &str) -> CommandError {
    CommandError::new(format!(
        "could not recover the interrupted overwrite of {}: {detail}; left the intent in place",
        intent.stem
    ))
}

fn state_ref_dir() -> PathBuf {
    bob_env::bob_cli_state_dir().join("ref")
}

fn canonical_vault(
    config: &Config,
) -> std::result::Result<String, CommandError> {
    fs::canonicalize(&config.bob_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            CommandError::new(format!(
                "could not resolve the vault path {}: {error}",
                config.bob_dir.display()
            ))
        })
}

/// File holding the pending intent for one canonical vault.
fn intent_path_for(state_dir: &Path, canonical_vault: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(canonical_vault.as_bytes());
    let hex = hex::encode(digest.finalize());
    state_dir
        .join(INTENT_DIR_NAME)
        .join(format!("overwrite-{hex}.json"))
}

fn intent_display_path(state_dir: &Path, canonical_vault: &str) -> PathBuf {
    intent_path_for(state_dir, canonical_vault)
}

fn save_intent_to(
    state_dir: &Path,
    config: &Config,
    intent: &OverwriteIntent,
) -> std::result::Result<PathBuf, String> {
    let canonical = fs::canonicalize(&config.bob_dir).map_err(|error| {
        format!("could not resolve the vault path: {error}")
    })?;
    let mut stored = intent.clone();
    stored.schema_version = INTENT_SCHEMA_VERSION;
    stored.vault = canonical.to_string_lossy().into_owned();
    let path = intent_path_for(state_dir, &stored.vault);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("could not create {}: {error}", parent.display())
        })?;
    }
    let text = serde_json::to_string(&stored).map_err(|error| {
        format!("could not serialize overwrite intent: {error}")
    })?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, text.as_bytes()).map_err(|error| {
        format!("could not write {}: {error}", temporary.display())
    })?;
    fs::File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            format!("could not fsync {}: {error}", temporary.display())
        })?;
    fs::rename(&temporary, &path).map_err(|error| {
        format!("could not install {}: {error}", path.display())
    })?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(path)
}

fn load_intent_from(
    state_dir: &Path,
    config: &Config,
) -> std::result::Result<Option<OverwriteIntent>, CommandError> {
    let canonical = canonical_vault(config)?;
    let path = intent_path_for(state_dir, &canonical);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "could not read overwrite intent {}: {error}; remove it to proceed",
                path.display()
            )));
        }
    };
    match serde_json::from_str::<OverwriteIntent>(&text) {
        Ok(intent) => Ok(Some(intent)),
        Err(error) => Err(CommandError::new(format!(
            "ignoring malformed overwrite intent at {} ({error}); remove it to proceed",
            path.display()
        ))),
    }
}

fn clear_intent_in(
    state_dir: &Path,
    canonical_vault: &str,
) -> std::result::Result<(), String> {
    let path = intent_path_for(state_dir, canonical_vault);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not clear overwrite intent {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const STEM: &str = "half-written";
    const REF_TYPE: &str = "blogs";
    const SUBJECT: &str = "bob ref create: overwrite blogs/half-written";

    fn run_git(vault: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_output(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(vault: &Path) {
        run_git(vault, &["init", "-q"]);
        run_git(vault, &["config", "user.email", "test@example.com"]);
        run_git(vault, &["config", "user.name", "Test"]);
        run_git(vault, &["config", "commit.gpgsign", "false"]);
    }

    fn test_config(vault: &Path) -> Config {
        Config {
            bob_dir: vault.to_path_buf(),
            lib_dir: vault.join("lib"),
            ref_dir: vault.join("ref"),
            xlib_dir: vault.join("xlib"),
        }
    }

    fn fixture_vault() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        for directory in ["lib/blogs", "ref/blogs", "xlib"] {
            fs::create_dir_all(vault.join(directory)).expect("vault dirs");
        }
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nbody\n",
        )
        .expect("note");
        fs::write(vault.join("lib/blogs/half-written.pdf"), b"pdf-bytes")
            .expect("pdf");
        init_repo(vault);
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", "fixture"]);
        temp
    }

    fn pending_intent(
        base_head: &str,
        expected_head: Option<&str>,
        touched: &[&str],
        created: &[&str],
    ) -> OverwriteIntent {
        OverwriteIntent {
            schema_version: INTENT_SCHEMA_VERSION,
            vault: String::new(),
            stem: STEM.to_string(),
            ref_type: REF_TYPE.to_string(),
            base_head: base_head.to_string(),
            expected_subject: SUBJECT.to_string(),
            expected_head: expected_head.map(str::to_string),
            touched: touched.iter().map(ToString::to_string).collect(),
            created: created.iter().map(ToString::to_string).collect(),
        }
    }

    fn save_pending(
        state: &Path,
        config: &Config,
        intent: &OverwriteIntent,
    ) -> OverwriteIntent {
        let mut stored = intent.clone();
        stored.vault = fs::canonicalize(&config.bob_dir)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        save_intent_to(state, config, &stored).expect("save intent");
        stored
    }

    fn intent_file(state: &Path, config: &Config) -> PathBuf {
        let canonical = fs::canonicalize(&config.bob_dir)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        intent_path_for(state, &canonical)
    }

    fn recover(
        state: &Path,
        config: &Config,
        intent: &OverwriteIntent,
        dry_run: bool,
    ) -> std::result::Result<Option<String>, CommandError> {
        let child_env = ob::child_env();
        recover_locked(state, config, &child_env, intent, dry_run)
    }

    #[test]
    fn rollback_restores_half_written_files_and_removes_creations() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        fs::write(vault.join("lib/blogs/half-written.pdf"), b"crashed-pdf")
            .expect("dirty pdf");
        fs::create_dir_all(vault.join("lib/blogs/half-written.textbundle"))
            .expect("sidecar dir");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/text.md"),
            "sidecar\n",
        )
        .expect("sidecar file");
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        fs::write(
            vault.join("ref_history/blogs/half-written__2026-10-26.md"),
            "frozen\n",
        )
        .expect("archive");
        // Unrelated dirty and staged content must survive scoped recovery.
        fs::write(vault.join("unrelated.md"), "unrelated-dirty\n")
            .expect("unrelated");
        run_git(vault, &["add", "unrelated.md"]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &["ref/blogs/half-written.md", "lib/blogs/half-written.pdf"],
                &[
                    "ref_history/blogs/half-written__2026-10-26.md",
                    "lib/blogs/half-written.textbundle/text.md",
                ],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: rolled back an interrupted overwrite of half-written")
        );
        assert_eq!(
            fs::read(vault.join("ref/blogs/half-written.md")).expect("note"),
            b"---\nid: half-written\n---\nbody\n"
        );
        assert_eq!(
            fs::read(vault.join("lib/blogs/half-written.pdf")).expect("pdf"),
            b"pdf-bytes"
        );
        assert!(
            !vault
                .join("ref_history/blogs/half-written__2026-10-26.md")
                .exists(),
            "the recorded archive creation is removed"
        );
        assert!(
            !vault.join("lib/blogs/half-written.textbundle").exists(),
            "the emptied sidecar directory is pruned"
        );
        assert!(
            !intent_file(state, &config).exists(),
            "the intent is cleared after a proved rollback"
        );
        assert_eq!(
            fs::read(vault.join("unrelated.md")).expect("unrelated"),
            b"unrelated-dirty\n"
        );
        assert_eq!(
            git_output(vault, &["diff", "--cached", "--name-only"]),
            "unrelated.md"
        );
        // Recovery is idempotent: the intent is gone, so a fresh load finds
        // nothing to do.
        assert!(load_intent_from(state, &config).expect("reload").is_none());
    }

    #[test]
    fn committed_intent_is_retained_and_cleared() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nreplaced\n",
        )
        .expect("new body");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);
        let committed = git_output(vault, &["rev-parse", "HEAD"]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                Some(&committed),
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("retain");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: kept the completed overwrite of half-written")
        );
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("replaced"),
            "the committed state is kept"
        );
        assert!(
            !intent_file(state, &config).exists(),
            "the intent is cleared once the commit is proved"
        );
    }

    #[test]
    fn committed_without_recorded_sha_still_retains() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The transaction committed but crashed before recording the SHA:
        // HEAD advanced past the base carrying the expected subject.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nreplaced\n",
        )
        .expect("new body");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let notice = recover(state, &config, &intent, false).expect("retain");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: kept the completed overwrite of half-written")
        );
        assert!(!intent_file(state, &config).exists());
    }

    #[test]
    fn previous_same_stem_commit_is_not_clobbered_by_rollback() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        // An earlier overwrite committed; the new overwrite crashed before
        // committing, so recovery must restore the earlier commit's state.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nfirst-overwrite\n",
        )
        .expect("first overwrite");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("crashed second overwrite");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("first-overwrite"),
            "rollback restores the previous overwrite, it does not erase it"
        );
    }

    #[test]
    fn moved_vault_preserves_the_intent_without_touching_files() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // An unrelated commit advanced HEAD past the base without the
        // expected subject: recovery must fail closed.
        fs::write(vault.join("unrelated.md"), "unrelated\n").expect("file");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", "unrelated work"]);
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");

        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let error =
            recover(state, &config, &intent, false).expect_err("moved vault");
        assert!(
            error.to_string().contains("left the intent in place"),
            "unexpected error: {error}"
        );
        assert!(
            intent_file(state, &config).exists(),
            "the marker is preserved when repair cannot proceed"
        );
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "no restore runs when HEAD moved"
        );
    }

    #[test]
    fn wrong_subject_expected_head_rolls_back() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The recorded SHA exists but carries another subject, so it cannot
        // prove this overwrite committed.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                Some(&base),
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert!(!fs::read_to_string(vault.join("ref/blogs/half-written.md"))
            .expect("note")
            .contains("CRASHED"));
    }

    #[test]
    fn dry_run_is_read_only_and_reports_recovery_required() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let error =
            recover(state, &config, &intent, true).expect_err("dry run");
        assert_eq!(error.code, Some(RECOVERY_REQUIRED_CODE));
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "dry runs restore nothing"
        );
        assert!(
            intent_file(state, &config).exists(),
            "dry runs clear nothing"
        );
    }

    #[test]
    fn invalid_paths_fail_closed() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        for invalid in
            ["../evil.md", "/absolute.md", "ref/blogs/../../evil.md", ""]
        {
            let intent = save_pending(
                state,
                &config,
                &pending_intent(&base, None, &[invalid], &[]),
            );
            let error =
                recover(state, &config, &intent, false).expect_err("invalid");
            assert!(
                error.to_string().contains("invalid path"),
                "unexpected error for {invalid:?}: {error}"
            );
            assert!(
                intent_file(state, &config).exists(),
                "invalid intents are preserved for manual cleanup"
            );
            assert!(!vault.join("evil.md").exists());
        }
        // `.git` roots are never restored or cleaned.
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &[".git/refs/heads/main"], &[]),
        );
        recover(state, &config, &intent, false).expect_err(".git refused");
        assert!(intent_file(state, &config).exists());
    }

    #[test]
    fn malformed_intent_fails_closed() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        let canonical = fs::canonicalize(vault)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        let path = intent_path_for(state, &canonical);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(&path, "{not json").expect("malformed marker");

        let error = load_intent_from(state, &config).expect_err("malformed");
        assert!(
            error.to_string().contains("malformed overwrite intent"),
            "unexpected error: {error}"
        );
        assert!(path.exists(), "malformed markers are never deleted");
    }

    #[test]
    fn bogus_base_preserves_the_intent() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                "0".repeat(40).as_str(),
                None,
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let error =
            recover(state, &config, &intent, false).expect_err("repair");
        assert!(
            error.to_string().contains("left the intent in place"),
            "unexpected error: {error}"
        );
        assert!(intent_file(state, &config).exists());
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "failed repairs leave the worktree alone"
        );
    }

    #[test]
    fn failed_checkout_preserves_the_intent() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // A touched path absent from the base fails the whole restore, so
        // the marker must survive for a retry and the worktree is untouched.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &["ref/blogs/half-written.md", "ref/blogs/no-such-file.md"],
                &[],
            ),
        );
        let error =
            recover(state, &config, &intent, false).expect_err("checkout");
        assert!(
            error.to_string().contains("could not restore vault files"),
            "unexpected error: {error}"
        );
        assert!(intent_file(state, &config).exists());
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "failed repairs leave the worktree alone"
        );
    }

    #[test]
    fn created_directories_keep_unrecorded_contents() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The transaction recorded only the sidecar file; a user file that
        // landed in the same directory afterwards must survive.
        fs::create_dir_all(vault.join("lib/blogs/half-written.textbundle"))
            .expect("sidecar dir");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/text.md"),
            "sidecar\n",
        )
        .expect("sidecar file");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/user-kept.md"),
            "user\n",
        )
        .expect("user file");

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &[],
                &[
                    "lib/blogs/half-written.textbundle",
                    "lib/blogs/half-written.textbundle/text.md",
                ],
            ),
        );
        recover(state, &config, &intent, false).expect("rollback");
        assert!(
            !vault
                .join("lib/blogs/half-written.textbundle/text.md")
                .exists(),
            "recorded creations are removed"
        );
        assert_eq!(
            fs::read(
                vault.join("lib/blogs/half-written.textbundle/user-kept.md")
            )
            .expect("user file"),
            b"user\n",
            "unrecorded contents survive directory cleanup"
        );
    }
}
