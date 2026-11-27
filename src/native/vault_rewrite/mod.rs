//! One shared vault-rewrite transaction for global renames.
//!
//! Extracted from `bob types rename` (`src/native/pomodoro_themes/rename.rs`)
//! so every vocabulary rename — themes today, relation labels next — runs the
//! same steps: online vault identity check (`vault_mismatch`), bounded
//! lock wait (`lock_busy`), the worktree check
//! (`not_a_worktree`), pre-sync unless offline (`sync_failed`), planning
//! through a caller closure over the synced files, before-image snapshots and
//! the stale check (`stale_note`), dirty-path rules (offline refuses with
//! `dirty_paths`; online notes pre-sync inclusion), a durable intent
//! journal with crash recovery (`recovery_pending`), atomic writes with
//! rollback, trash removes that move the file into `.trash/`, no-replace
//! creates that never overwrite a file that appeared after planning, a
//! post-write hook for the journal append, a verify hook that restores on
//! failure (`verify_failed`), one commit with the caller's subject,
//! unstaging on failure, and committed-despite-error handling, then
//! post-sync, with "sync pending" reported separately.
//!
//! The caller owns everything domain-specific: planning and validation,
//! the journal event, verification, the success report, and all output. The
//! failure codes and message shapes below are the `bob types rename`
//! contract, kept so that move stays byte-for-byte.

mod recovery;

pub(crate) use recovery::recover_pending;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::native::{
    collect_done,
    ob::{self, LockWaitError},
    periods,
    ref_library::migrate_tasks,
    style::Styler,
    vault_sync,
};

/// Bounded wait for `bob_sync.lock` before any vault rewrite.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// One transaction failure: a stable machine-readable code, the human
/// message, and an optional retry hint. Codes are the rename contract
/// (`vault_mismatch`, `lock_busy`, `not_a_worktree`, `sync_failed`, `stale_note`,
/// `dirty_paths`, `verify_failed`, plus `recovery_pending` for an
/// interrupted rewrite recovery could not roll back, plus `io` for the Git
/// status read).
/// `kind` is an additive machine-readable failure kind (`"network"` when
/// the vault remote was unreachable); `None` preserves the old untyped
/// shape.
#[derive(Debug, Clone)]
pub(crate) struct TxnFailure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) kind: Option<&'static str>,
}

impl TxnFailure {
    pub(crate) fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
            kind: None,
        }
    }

    /// Attach a machine-readable kind (for example `"network"`).
    pub(crate) fn with_kind(mut self, kind: &'static str) -> Self {
        self.kind = Some(kind);
        self
    }
}

/// One planned file rewrite, create, or trash: vault-relative `/`-joined
/// path plus the exact before-image the planner saw and the after-image to
/// write. `before_contents` is `Some` for a rewrite or trash (the stale
/// check compares the live file against it) and `None` for a create (the
/// stale check requires the target to still be absent). `trash` moves the
/// file into the vault's `.trash/` instead of writing it; `after_contents`
/// is ignored for trash entries (use `FileRewrite::trash`).
#[derive(Debug, Clone)]
pub(crate) struct FileRewrite {
    pub(crate) path: String,
    pub(crate) before_contents: Option<String>,
    pub(crate) after_contents: String,
    pub(crate) trash: bool,
}

/// The caller plan a transaction runs: the file rewrites, the single commit
/// subject, the post-write hook (journal append; its `Ok` paths join the
/// commit, its `Err` message becomes the `verify_failed` detail), the
/// verify hook (rescan; its `Err` detail becomes the `verify_failed`
/// detail), the post-commit hook (cache invalidation), and opaque caller
/// context (report inputs) returned untouched in the outcome.
pub(crate) struct ReadyPlan<C> {
    pub(crate) files: Vec<FileRewrite>,
    pub(crate) subject: String,
    pub(crate) post_write: Box<dyn FnOnce() -> Result<Vec<PathBuf>, String>>,
    pub(crate) verify: Box<dyn FnOnce() -> Result<(), String>>,
    pub(crate) on_committed: Box<dyn FnOnce()>,
    pub(crate) context: C,
}

/// A committed transaction: the new SHA and subject, the pre/post sync
/// states (`"ok"` or `"skipped"`, post also `"failed"`), the optional
/// dirty-inclusion warning, the optional post-sync-failure warning, the
/// optional warning from recovering an interrupted rewrite at the start of
/// the run, and the caller's untouched context.
#[derive(Debug)]
pub(crate) struct TxnOutcome<C> {
    pub(crate) sha: String,
    pub(crate) subject: String,
    pub(crate) sync_pre: &'static str,
    pub(crate) sync_post: &'static str,
    pub(crate) post_sync_warning: Option<String>,
    pub(crate) dirty_warning: Option<String>,
    pub(crate) recovery_warning: Option<String>,
    pub(crate) context: C,
}

/// Run one vault-rewrite transaction: lock, interrupted-rewrite recovery,
/// worktree check, pre-sync, plan, snapshot and stale check, dirty rules,
/// intent journal, atomic writes, post-write hook, verify hook, one commit,
/// post-commit hook, and post-sync. A crash between the journal persist and
/// the intent removal leaves the intent for the next run (or the vault-sync
/// pre-cycle) to roll back or refuse with `recovery_pending`.
pub(crate) fn run<C>(
    vault: &Path,
    child_env: &ob::ChildEnv,
    command_name: &'static str,
    worktree_subject: &str,
    offline: bool,
    json_mode: bool,
    plan: impl FnOnce() -> Result<ReadyPlan<C>, TxnFailure>,
) -> Result<TxnOutcome<C>, TxnFailure> {
    if !offline && let Some(detail) = vault_sync::vault_identity_problem(vault)
    {
        return Err(TxnFailure::new(
            "vault_mismatch",
            detail,
            Some("point BOB_DIR at the vault passed with -b, or use --offline"),
        ));
    }
    // Take bob_sync.lock first (wait at most 3s like the other writers).
    let _lock = {
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!(
                    "  {}",
                    Styler::detect()
                        .dim("waiting for another vault maintenance run…")
                );
            }
        };
        match ob::acquire_lock_waiting(LOCK_WAIT, on_first_wait) {
            Ok(lock) => lock,
            Err(error) => {
                return Err(match error {
                    LockWaitError::Timeout { path } => TxnFailure::new(
                        "lock_busy",
                        format!(
                            "another Bob vault maintenance run is already active (waited on {})",
                            path.display()
                        ),
                        Some("wait for the other maintenance run to finish, then retry"),
                    ),
                    other => TxnFailure::new(
                        "lock_busy",
                        format!("could not acquire the vault maintenance lock: {other}"),
                        Some("wait for the other maintenance run to finish, then retry"),
                    ),
                });
            }
        }
    };

    // Recover an interrupted rewrite under the lock before touching the
    // vault: a rolled-back intent reports one warning, an unrecoverable
    // one refuses before any new write.
    let recovery_warning = match recovery::recover_pending(vault, child_env) {
        Ok(warning) => {
            if let Some(notice) = warning.as_deref() {
                eprintln!("{command_name}: {notice}");
            }
            warning
        }
        Err(failure) => return Err(failure),
    };

    match ob::detect_git_worktree(vault, child_env) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return Err(TxnFailure::new(
                "not_a_worktree",
                format!(
                    "{worktree_subject} require a Git worktree: {}",
                    vault.display()
                ),
                Some("initialize or open the Bob vault as a Git worktree"),
            ));
        }
    }

    if !offline {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(child_env, true);
        if !cycle.ok || !cycle.conflicts.is_empty() {
            let detail = if cycle.conflicts.is_empty() {
                cycle.error.unwrap_or_else(|| {
                    "vault sync reported failure".to_string()
                })
            } else {
                format!("vault sync conflict in {}", cycle.conflicts.join(", "))
            };
            return Err(TxnFailure::new(
                "sync_failed",
                format!("pre-sync failed: {detail}"),
                Some("re-run with --offline to commit locally without syncing"),
            ));
        }
    }

    let mut ready = plan()?;
    // Mechanical rewrites carry eligible Review and summary digests from the
    // original note so bob's own edits never read as Bryan's. Trash entries
    // move the file untouched, so they skip the digest carry.
    for file in &mut ready.files {
        if !file.trash
            && let Some(before) = file.before_contents.as_deref()
        {
            let subject = crate::native::intentions::fields::canonical_subject(
                &file.path,
            );
            file.after_contents = periods::note::carry_rewrite_digests(
                before,
                &file.after_contents,
                subject,
            )
            .map_err(|error| {
                TxnFailure::new(
                    error.code,
                    format!(
                        "could not carry draft digests in {}: {}",
                        file.path, error.message
                    ),
                    Some("repair the note's frontmatter, then re-run the rewrite"),
                )
            })?;
        }
    }
    let touched_relative: BTreeSet<PathBuf> = ready
        .files
        .iter()
        .map(|file| PathBuf::from(&file.path))
        .collect();

    // Snapshot each target and compare again right before writing.
    let relatives: Vec<String> =
        ready.files.iter().map(|file| file.path.clone()).collect();
    let before_images = snapshot_before_images(vault, &relatives);
    // Re-read to detect a change since planning. Creates must still be
    // absent; rewrites must still match the planner's before-image.
    for file in &ready.files {
        let absolute = vault.join(&file.path);
        let stale = match &file.before_contents {
            Some(expected) => {
                let current = fs::read_to_string(&absolute).unwrap_or_default();
                current != *expected
            }
            None => absolute.exists(),
        };
        if stale {
            let _ = restore_before_images(
                vault,
                &before_images,
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
            return Err(TxnFailure::new(
                "stale_note",
                format!(
                    "{} changed since planning; retry the rename",
                    file.path
                ),
                Some(format!(
                    "re-run {command_name} to replan from the latest files"
                )),
            ));
        }
    }

    // Dirty paths abort an offline rewrite; online they ride the pre-sync.
    let dirty = dirty_touched_paths(vault, &touched_relative)?;
    if offline && !dirty.is_empty() {
        return Err(TxnFailure::new(
            "dirty_paths",
            format!("rename paths are dirty: {}", dirty.join(", ")),
            Some("commit or restore those paths before an offline rename"),
        ));
    }
    let dirty_warning = if dirty.is_empty() {
        None
    } else {
        Some(format!(
            "dirty paths will be included by the pre-sync: {}",
            dirty.join(", ")
        ))
    };

    let old_head = git_head(vault, child_env).ok();
    // Snapshot the relation-label journal before the post-write hook so a
    // rename event never survives a rolled-back transaction, including when
    // the host journal did not exist before the append.
    let journal_before = snapshot_journal(vault);
    // Persist the crash intent before the first write: every path with its
    // action, before/after hashes, before-image copies, and trash
    // destinations, all fsynced. Recovery after a crash reads only this.
    let persisted =
        persist_txn_intent(vault, &ready, &before_images, old_head.as_deref())?;
    let trash_dests = trash_destinations(vault, &ready.files, &persisted);
    // Write each file atomically, then run the post-write hook. A simulated
    // crash (test-only fault point) returns without any cleanup so the
    // intent stays for recovery, exactly like a killed process.
    if let Err(failure) = write_planned_files(
        vault,
        &ready.files,
        &before_images,
        &trash_dests,
        &persisted,
        command_name,
    ) {
        if failure.code == CRASH_INJECTED_CODE {
            return Err(failure);
        }
        recovery::clear_intent(&persisted.run_dir);
        return Err(failure);
    }
    let extra_paths = match (ready.post_write)() {
        Ok(paths) => paths,
        Err(message) => {
            let _ = restore_before_images(
                vault,
                &before_images,
                &after_images(vault, &ready.files),
                &trash_dests,
            );
            restore_journal(vault, &journal_before);
            recovery::clear_intent(&persisted.run_dir);
            return Err(TxnFailure::new(
                "verify_failed",
                message,
                Some("the rename was rolled back; inspect the listed paths and retry"),
            ));
        }
    };

    // Verify by rescanning; on failure restore the before-images.
    if let Err(message) = (ready.verify)() {
        let _ = restore_before_images(
            vault,
            &before_images,
            &after_images(vault, &ready.files),
            &trash_dests,
        );
        restore_journal(vault, &journal_before);
        recovery::clear_intent(&persisted.run_dir);
        return Err(TxnFailure::new(
            "verify_failed",
            format!("could not apply and verify the rename: {message}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        ));
    }

    let mut commit_paths = ready
        .files
        .iter()
        .map(|file| PathBuf::from(&file.path))
        .collect::<Vec<_>>();
    commit_paths.extend(extra_paths);
    commit_paths.sort();
    commit_paths.dedup();
    let sha = match ob::commit_paths(
        vault,
        child_env,
        &ready.subject,
        &commit_paths,
    ) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            let unstage = unstage_paths(vault, child_env, &commit_paths);
            let _ = restore_before_images(
                vault,
                &before_images,
                &after_images(vault, &ready.files),
                &trash_dests,
            );
            restore_journal(vault, &journal_before);
            recovery::clear_intent(&persisted.run_dir);
            return Err(TxnFailure::new(
                "verify_failed",
                format!("rename verified but produced no scoped Git commit{unstage}"),
                Some("check the vault Git state and retry"),
            ));
        }
        Err(error) => {
            if let Some((sha, committed_subject)) = committed_despite_error(
                vault,
                child_env,
                old_head.as_deref(),
                &ready.subject,
            ) {
                if committed_subject == ready.subject {
                    let _ = error;
                    sha
                } else {
                    let unstage =
                        unstage_paths(vault, child_env, &commit_paths);
                    let _ = restore_before_images(
                        vault,
                        &before_images,
                        &after_images(vault, &ready.files),
                        &trash_dests,
                    );
                    restore_journal(vault, &journal_before);
                    recovery::clear_intent(&persisted.run_dir);
                    return Err(TxnFailure::new(
                        "verify_failed",
                        format!("Git commit failed: {error}{unstage}"),
                        Some("check the vault Git state and retry"),
                    ));
                }
            } else {
                let unstage = unstage_paths(vault, child_env, &commit_paths);
                let _ = restore_before_images(
                    vault,
                    &before_images,
                    &after_images(vault, &ready.files),
                    &trash_dests,
                );
                restore_journal(vault, &journal_before);
                recovery::clear_intent(&persisted.run_dir);
                return Err(TxnFailure::new(
                    "verify_failed",
                    format!("Git commit failed: {error}{unstage}"),
                    Some("check the vault Git state and retry"),
                ));
            }
        }
    };

    // Record the commit in the intent so a crash from here until the
    // removal still proves the commit, then remove the intent. A record
    // failure never fails a committed transaction: the next run proves
    // the commit by subject and tree instead.
    if let Err(failure) = recovery::record_commit(&persisted.run_dir, &sha) {
        eprintln!("{command_name}: warning: {}", failure.message);
    }
    #[cfg(test)]
    if take_crash_after_commit() {
        return Err(TxnFailure::new(
            CRASH_INJECTED_CODE,
            "injected crash after the commit",
            None::<String>,
        ));
    }
    recovery::clear_intent(&persisted.run_dir);

    (ready.on_committed)();

    let sync_pre = if offline { "skipped" } else { "ok" };
    let (sync_post, post_sync_warning) = if offline {
        ("skipped", None)
    } else {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(child_env, true);
        if !cycle.ok || !cycle.conflicts.is_empty() {
            let detail = if cycle.conflicts.is_empty() {
                cycle.error.unwrap_or_else(|| {
                    "vault sync reported failure".to_string()
                })
            } else {
                format!("vault sync conflict in {}", cycle.conflicts.join(", "))
            };
            (
                "failed",
                Some(format!(
                    "the rename is committed locally; post-sync failed ({detail}); run bob vault-sync"
                )),
            )
        } else {
            ("ok", None)
        }
    };
    Ok(TxnOutcome {
        sha,
        subject: ready.subject,
        sync_pre,
        sync_post,
        post_sync_warning,
        dirty_warning,
        recovery_warning,
        context: ready.context,
    })
}

/// Crash code for the test-only fault point: it stops mid-write and
/// skips every cleanup so the intent stays for recovery, exactly like a
/// killed process.
const CRASH_INJECTED_CODE: &str = "crash_injected";

/// Build the crash intent for one transaction: every path with its action
/// and live before/after bytes, plus the recorded base HEAD.
fn persist_txn_intent<C>(
    vault: &Path,
    ready: &ReadyPlan<C>,
    before_images: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    base_head: Option<&str>,
) -> Result<recovery::PersistedIntent, TxnFailure> {
    let mut pending = Vec::with_capacity(ready.files.len());
    for file in &ready.files {
        let absolute = vault.join(&file.path);
        let before_bytes = before_images
            .get(&absolute)
            .and_then(|bytes| bytes.as_deref());
        if file.trash && before_bytes.is_none() {
            return Err(TxnFailure::new(
                "verify_failed",
                format!("cannot trash {} without a before-image", file.path),
                Some("replan the rewrite and retry"),
            ));
        }
        let (action, after_bytes) = if file.trash {
            (recovery::JournalAction::Trash, None)
        } else if file.before_contents.is_some() {
            (
                recovery::JournalAction::Rewrite,
                Some(file.after_contents.as_bytes()),
            )
        } else {
            (
                recovery::JournalAction::Create,
                Some(file.after_contents.as_bytes()),
            )
        };
        pending.push(recovery::PendingJournalFile {
            path: &file.path,
            action,
            before_bytes,
            after_bytes,
        });
    }
    recovery::persist_intent(vault, &ready.subject, base_head, &pending)
}

/// Absolute trash destinations keyed by absolute original path, for the
/// files this transaction trashes.
fn trash_destinations(
    vault: &Path,
    files: &[FileRewrite],
    persisted: &recovery::PersistedIntent,
) -> BTreeMap<PathBuf, PathBuf> {
    let mut map = BTreeMap::new();
    for (file, journaled) in files.iter().zip(persisted.files.iter()) {
        if file.trash
            && let Some(destination) = journaled.trash_destination.as_deref()
        {
            map.insert(vault.join(&file.path), vault.join(destination));
        }
    }
    map
}

/// Write each planned file, re-checking staleness immediately before its
/// own write. Tracks every installed after-image so a stale file or a write
/// failure rolls back earlier writes in the same loop. Rewrites install
/// atomically, creates install through a hard link so a file that appeared
/// after the stale check is never overwritten, and trash entries move into
/// `.trash/`.
fn write_planned_files(
    vault: &Path,
    files: &[FileRewrite],
    before_images: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    trash_dests: &BTreeMap<PathBuf, PathBuf>,
    persisted: &recovery::PersistedIntent,
    command_name: &'static str,
) -> Result<(), TxnFailure> {
    let mut written: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut installed_trash: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    // Re-compare each file immediately before its own write: a concurrent
    // Obsidian save landing between the bulk stale check and this file's
    // write must fail through `stale_note`, not be overwritten.
    for (index, file) in files.iter().enumerate() {
        #[cfg(test)]
        if take_crash_before_write() {
            return Err(TxnFailure::new(
                CRASH_INJECTED_CODE,
                "injected crash mid-write",
                None::<String>,
            ));
        }
        let absolute = vault.join(&file.path);
        let stale_now = match &file.before_contents {
            Some(expected) => {
                fs::read_to_string(&absolute).unwrap_or_default() != *expected
            }
            None => absolute.exists(),
        };
        if stale_now {
            let rollback = restore_before_images(
                vault,
                before_images,
                &written,
                &installed_trash,
            );
            return Err(TxnFailure::new(
                "stale_note",
                format!(
                    "{} changed since planning; retry the rename{rollback}",
                    file.path
                ),
                Some(format!(
                    "re-run {command_name} to replan from the latest files"
                )),
            ));
        }
        if file.trash {
            let installed = install_trash(
                vault,
                &absolute,
                &file.path,
                index,
                persisted,
                trash_dests,
            )?;
            installed_trash.insert(absolute, installed);
        } else if file.before_contents.is_none() {
            if let Err(error) = install_create(&absolute, &file.after_contents)
            {
                let rollback = restore_before_images(
                    vault,
                    before_images,
                    &written,
                    &installed_trash,
                );
                return Err(match error {
                    CreateError::Appeared => TxnFailure::new(
                        "stale_note",
                        format!(
                            "{} changed since planning; retry the rename{rollback}",
                            file.path
                        ),
                        Some(format!(
                            "re-run {command_name} to replan from the latest files"
                        )),
                    ),
                    CreateError::Failed(detail) => TxnFailure::new(
                        "verify_failed",
                        format!("could not write {}: {detail}{rollback}", file.path),
                        Some(
                            "the rename was rolled back; inspect the listed paths and retry",
                        ),
                    ),
                });
            }
            written.insert(absolute, file.after_contents.as_bytes().to_vec());
        } else if let Err(error) =
            collect_done::atomic_write(&absolute, &file.after_contents)
        {
            let rollback = restore_before_images(
                vault,
                before_images,
                &written,
                &installed_trash,
            );
            return Err(TxnFailure::new(
                "verify_failed",
                format!("could not write {}: {error}{rollback}", file.path),
                Some("the rename was rolled back; inspect the listed paths and retry"),
            ));
        } else {
            written.insert(absolute, file.after_contents.as_bytes().to_vec());
        }
    }
    #[cfg(test)]
    if take_crash_before_write() {
        return Err(TxnFailure::new(
            CRASH_INJECTED_CODE,
            "injected crash after the writes",
            None::<String>,
        ));
    }
    Ok(())
}

/// Move one file into the vault trash, using the intent's pre-assigned
/// destination. On an install-time collision (a file appeared at the
/// destination after the intent was persisted), the next free Obsidian
/// suffix wins and the intent records the actual destination.
fn install_trash(
    vault: &Path,
    absolute: &Path,
    relative: &str,
    index: usize,
    persisted: &recovery::PersistedIntent,
    trash_dests: &BTreeMap<PathBuf, PathBuf>,
) -> Result<PathBuf, TxnFailure> {
    let mut destination =
        trash_dests.get(absolute).cloned().unwrap_or_else(|| {
            let fallback = vault.join(format!(".trash/{relative}"));
            if let Some(parent) = fallback.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fallback
        });
    if destination.exists() {
        let file_name = absolute
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("note.md");
        let (fresh, fresh_relative) =
            recovery::pick_trash_destination(vault, file_name)?;
        recovery::record_trash_destination(
            &persisted.run_dir,
            index,
            &fresh_relative,
        )?;
        destination = fresh;
    }
    fs::rename(absolute, &destination).map_err(|error| {
        TxnFailure::new(
            "verify_failed",
            format!("could not trash {relative}: {error}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        )
    })?;
    Ok(destination)
}

#[derive(Debug)]
enum CreateError {
    /// The target appeared after planning: refuse, never overwrite.
    Appeared,
    Failed(String),
}

/// Install a create without replacing a path that appeared after the
/// stale check. Prefer `hard_link(temp, target)` then unlink the temp
/// (the link fails when the target exists); when hard links are
/// unsupported, fall back to `create_new`. Mirrors
/// `capture::commit::install_new_file`.
fn install_create(absolute: &Path, contents: &str) -> Result<(), CreateError> {
    if let Some(parent) = absolute.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CreateError::Failed(format!(
                "could not create {}: {error}",
                parent.display()
            ))
        })?;
    }
    let file_name = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note.md");
    let temp = absolute
        .with_file_name(format!("{file_name}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    fs::write(&temp, contents).map_err(|error| {
        CreateError::Failed(format!(
            "could not stage {}: {error}",
            temp.display()
        ))
    })?;
    if fs::File::open(&temp)
        .and_then(|file| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temp);
        return Err(CreateError::Failed(format!(
            "could not sync {}",
            temp.display()
        )));
    }
    #[cfg(test)]
    if force_create_new() {
        let result = install_create_new(absolute, contents);
        let _ = fs::remove_file(&temp);
        return result;
    }
    match fs::hard_link(&temp, absolute) {
        Ok(()) => {
            let _ = fs::remove_file(&temp);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temp);
            Err(CreateError::Appeared)
        }
        Err(error) if hard_link_unsupported(&error) => {
            let result = install_create_new(absolute, contents);
            let _ = fs::remove_file(&temp);
            result
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(CreateError::Failed(format!(
                "could not install {}: {error}",
                absolute.display()
            )))
        }
    }
}

fn install_create_new(
    absolute: &Path,
    contents: &str,
) -> Result<(), CreateError> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(absolute)
    {
        Ok(mut file) => {
            use std::io::Write as _;
            if let Err(error) = file
                .write_all(contents.as_bytes())
                .and_then(|()| file.sync_all())
            {
                let _ = fs::remove_file(absolute);
                return Err(CreateError::Failed(format!(
                    "could not install {}: {error}",
                    absolute.display()
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(CreateError::Appeared)
        }
        Err(error) => Err(CreateError::Failed(format!(
            "could not install {}: {error}",
            absolute.display()
        ))),
    }
}

fn hard_link_unsupported(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::Unsupported
        | ErrorKind::PermissionDenied
        | ErrorKind::CrossesDevices => true,
        _ => matches!(
            error.raw_os_error(),
            Some(code)
                if code == libc::EPERM
                    || code == libc::EXDEV
                    || code == libc::ENOTSUP
        ),
    }
}

#[cfg(test)]
thread_local! {
    static CRASH_BEFORE_WRITE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
    static CRASH_AFTER_COMMIT: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    static FORCE_CREATE_NEW: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Crash after k writes (test-only): `Some(0)` crashes before the first
/// write, `Some(n)` after n writes, including one final check after the
/// last write so `Some(n)` on n writes crashes before the commit.
#[cfg(test)]
pub(crate) fn set_crash_before_write(remaining: Option<usize>) {
    CRASH_BEFORE_WRITE.with(|slot| slot.set(remaining));
}

/// Crash after the commit is recorded but before the intent is removed
/// (test-only).
#[cfg(test)]
pub(crate) fn set_crash_after_commit(crash: bool) {
    CRASH_AFTER_COMMIT.with(|slot| slot.set(crash));
}

/// Force the `create_new` fallback for creates (test-only).
#[cfg(test)]
pub(crate) fn set_force_create_new(force: bool) {
    FORCE_CREATE_NEW.with(|slot| slot.set(force));
}

#[cfg(test)]
fn take_crash_before_write() -> bool {
    CRASH_BEFORE_WRITE.with(|slot| match slot.get() {
        None => false,
        Some(0) => true,
        Some(n) => {
            slot.set(Some(n - 1));
            false
        }
    })
}

#[cfg(test)]
fn take_crash_after_commit() -> bool {
    CRASH_AFTER_COMMIT.with(|slot| slot.replace(false))
}

#[cfg(test)]
fn force_create_new() -> bool {
    FORCE_CREATE_NEW.with(|slot| slot.get())
}

fn snapshot_before_images(
    vault: &Path,
    relatives: &[String],
) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    for relative in relatives {
        let absolute = vault.join(relative);
        let bytes = fs::read(&absolute).ok();
        map.insert(absolute, bytes);
    }
    map
}

fn after_images(
    vault: &Path,
    files: &[FileRewrite],
) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut map = BTreeMap::new();
    for file in files {
        // Trash installs leave absence behind, which the restore helper
        // reads as "nothing written": trash paths restore from the trash
        // destination instead.
        if !file.trash {
            map.insert(
                vault.join(&file.path),
                file.after_contents.as_bytes().to_vec(),
            );
        }
    }
    map
}

fn snapshot_journal(vault: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    let dir = vault.join(".bob/relations");
    let Ok(entries) = fs::read_dir(&dir) else {
        return map;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        map.insert(path.clone(), fs::read(&path).ok());
    }
    map
}

fn restore_journal(vault: &Path, before: &BTreeMap<PathBuf, Option<Vec<u8>>>) {
    // Only touch files we snapshotted plus any new journal file created
    // by the post-write hook. A concurrent external edit that no longer
    // matches the post-write bytes is left in place.
    let dir = vault.join(".bob/relations");
    let mut candidates: BTreeSet<PathBuf> = before.keys().cloned().collect();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                candidates.insert(path);
            }
        }
    }
    for absolute in candidates {
        let current = fs::read(&absolute).ok();
        let wanted = before.get(&absolute).cloned().unwrap_or(None);
        // If the file did not exist before and still does not, nothing to do.
        // If it did not exist before and now exists, remove it only when it
        // contains exactly the post-write state (we cannot know the delta,
        // so remove any file that was absent before: the transaction owns
        // journal appends and no other writer appends under our lock).
        match (wanted, current) {
            (None, None) => {}
            (None, Some(_)) => {
                let _ = fs::remove_file(&absolute);
            }
            (Some(previous), _) => {
                let _ = fs::write(&absolute, previous);
            }
        }
    }
}

fn restore_before_images(
    vault: &Path,
    before: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
    trash_dests: &BTreeMap<PathBuf, PathBuf>,
) -> String {
    // Trash originals restore from their trash destination; keep them out
    // of the shared content restore so absence is never read as a rewrite.
    let plain_before: BTreeMap<PathBuf, Option<Vec<u8>>> = before
        .iter()
        .filter(|(absolute, _)| !trash_dests.contains_key(*absolute))
        .map(|(absolute, bytes)| (absolute.clone(), bytes.clone()))
        .collect();
    let rels = plain_before
        .keys()
        .filter_map(|absolute| {
            absolute
                .strip_prefix(vault)
                .ok()
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect::<Vec<_>>();
    let before_map = migrate_tasks::snapshot_before_images(vault, &rels);
    // Reuse the shared restore helper for file contents; it reports
    // diverged paths that were left in place.
    let diverged = migrate_tasks::restore_before_images(&before_map, after);
    // The helper above snapshots current files, so restore the captured
    // before bytes directly for determinism.
    let mut failures: Vec<String> = diverged
        .into_iter()
        .map(|path| path.display().to_string())
        .collect();
    for (absolute, bytes) in &plain_before {
        let Ok(current) = fs::read(absolute) else {
            continue;
        };
        let wrote = after.get(absolute);
        if wrote.is_some_and(|wrote| &current == wrote) {
            let result = match bytes {
                Some(previous) => fs::write(absolute, previous),
                None => fs::remove_file(absolute),
            };
            if let Err(error) = result {
                failures.push(format!(
                    "could not restore {}: {error}",
                    absolute.display()
                ));
            }
        }
    }
    // Move trashed files back while the trash copy still holds the
    // before-image; anything else is a divergence to report.
    for (absolute, destination) in trash_dests {
        let before_bytes =
            before.get(absolute).and_then(|bytes| bytes.as_ref());
        match (fs::read(absolute), before_bytes) {
            (Ok(_), _) => {
                // The original is back (or was never moved): leave the
                // trash copy for Obsidian to show.
            }
            (Err(_), Some(previous)) => match fs::read(destination) {
                Ok(trashed) if &trashed == previous => {
                    if let Err(error) = fs::rename(destination, absolute) {
                        failures.push(format!(
                            "could not restore {}: {error}",
                            absolute.display()
                        ));
                    }
                }
                _ => {
                    failures.push(format!(
                        "could not restore {}: the trash copy changed",
                        absolute.display()
                    ));
                }
            },
            (Err(_), None) => {
                failures.push(format!(
                    "could not restore {}: no before-image",
                    absolute.display()
                ));
            }
        }
    }
    if failures.is_empty() {
        String::new()
    } else {
        format!("; rollback issues: {}", failures.join("; "))
    }
}

pub(crate) fn dirty_touched_paths(
    vault: &Path,
    touched: &BTreeSet<PathBuf>,
) -> Result<Vec<String>, TxnFailure> {
    if !vault.join(".git").exists() {
        return Ok(Vec::new());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(vault)
        .arg("--literal-pathspecs")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .map_err(|error| {
            TxnFailure::new(
                "io",
                format!("could not inspect Git status: {error}"),
                Some("check Git availability and retry"),
            )
        })?;
    if !output.status.success() {
        return Err(TxnFailure::new(
            "io",
            "could not inspect Git status for the rename plan",
            Some("check that the Bob vault is a Git worktree and retry"),
        ));
    }
    let mut records = output.stdout.split(|byte| *byte == 0);
    let mut dirty = BTreeSet::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]);
        let path =
            PathBuf::from(String::from_utf8_lossy(&record[3..]).as_ref());
        dirty.insert(path);
        if (status.contains('R') || status.contains('C'))
            && let Some(previous) = records.next()
        {
            dirty.insert(PathBuf::from(
                String::from_utf8_lossy(previous).as_ref(),
            ));
        }
    }
    let touched = touched
        .iter()
        .map(|path| normalized_path(&vault_path(path)))
        .collect::<BTreeSet<_>>();
    let mut matches = dirty
        .into_iter()
        .filter(|path| touched.contains(&normalized_path(&vault_path(path))))
        .map(|path| vault_path(&path))
        .collect::<Vec<_>>();
    matches.sort();
    Ok(matches)
}

fn vault_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

pub(crate) fn git_head(
    vault: &Path,
    child_env: &ob::ChildEnv,
) -> Result<String, String> {
    let output = ob::git_command(vault, child_env)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("git rev-parse HEAD: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn unstage_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: &[PathBuf],
) -> String {
    let output = ob::git_command_literal(vault, child_env)
        .arg("reset")
        .arg("--quiet")
        .arg("HEAD")
        .arg("--")
        .args(paths)
        .output();
    match output {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => format!(
            "; could not unstage rename paths: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("; could not unstage rename paths: {error}"),
    }
}

pub(crate) fn committed_despite_error(
    vault: &Path,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    expected_subject: &str,
) -> Option<(String, String)> {
    let output = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%H%n%s"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let mut lines = text.lines();
    let sha = lines.next()?.to_string();
    let subject = lines.next().unwrap_or("").to_string();
    (old_head != Some(sha.as_str()) && subject == expected_subject)
        .then_some((sha, subject))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    use crate::native::env::TestEnvGuard;

    const COMMAND: &str = "bob types rename";
    const SUBJECT: &str = "theme renames";

    fn git(vault: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_lines(vault: &Path, args: &[&str]) -> Vec<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A scratch Git vault with one committed note. Returns the temp dir
    /// (kept alive by the caller), the vault path, and a thread-local env
    /// guard pinning the maintenance lock to a vault-local file.
    fn scratch_vault(tag: &str) -> (tempfile::TempDir, PathBuf, TestEnvGuard) {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault dir");
        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "t@t"]);
        git(&vault, &["config", "user.name", "t"]);
        fs::write(vault.join("note.md"), "hello\n").expect("seed note");
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-qm", "init"]);
        let lock = temp.path().join(format!("{tag}.lock"));
        // Isolate the intent journal too: every transaction persists one,
        // and tests must never touch the real `~/.local/state`.
        let state = temp.path().join(format!("{tag}.state"));
        fs::create_dir_all(&state).expect("state dir");
        let guard = TestEnvGuard::set(&[
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
            ("XDG_STATE_HOME", Some(state.as_os_str())),
        ]);
        (temp, vault, guard)
    }

    /// Extra BOB_DIR + sync-state overrides for the online (syncing) tests.
    /// The state file lives beside the vault so the pre-sync cycle never
    /// stages it into the vault itself.
    fn sync_env(vault: &Path) -> TestEnvGuard {
        let state = vault.parent().unwrap_or(vault).join("sync-state.json");
        TestEnvGuard::set(&[
            ("BOB_DIR", Some(vault.as_os_str())),
            ("BOB_VAULT_SYNC_STATE_FILE", Some(state.as_os_str())),
            (
                "BOB_CONFIG_FILE",
                Some(OsStr::new("/nonexistent-bob-cli-test-config.yml")),
            ),
        ])
    }

    #[test]
    fn unstage_paths_leaves_a_glob_matching_sibling_staged() {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path();
        git(vault, &["init", "-q"]);
        git(vault, &["config", "user.email", "t@t"]);
        git(vault, &["config", "user.name", "t"]);
        fs::write(vault.join("What is Rust?.md"), "old named\n")
            .expect("named note");
        fs::write(vault.join("What is RustX.md"), "old sibling\n")
            .expect("sibling note");
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-qm", "init"]);
        fs::write(vault.join("What is Rust?.md"), "new named\n")
            .expect("edit named note");
        fs::write(vault.join("What is RustX.md"), "new sibling\n")
            .expect("edit sibling note");
        git(vault, &["add", "-A"]);

        let report = unstage_paths(
            vault,
            &ob::child_env(),
            &[PathBuf::from("What is Rust?.md")],
        );

        assert_eq!(report, "", "unstage succeeds");
        assert_eq!(
            git_lines(vault, &["diff", "--cached", "--name-only"]),
            vec!["What is RustX.md".to_string()],
            "unstaging the question-mark filename must not unstage its glob sibling"
        );
    }

    fn one_file_plan(before: &str, after: &str) -> Vec<FileRewrite> {
        vec![FileRewrite {
            path: "note.md".to_string(),
            before_contents: Some(before.to_string()),
            after_contents: after.to_string(),
            trash: false,
        }]
    }

    fn ok_hooks(
        verify: impl FnOnce() -> Result<(), String> + 'static,
    ) -> (ReadyPlan<()>, std::rc::Rc<std::cell::Cell<bool>>) {
        let committed = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = std::rc::Rc::clone(&committed);
        let plan = ReadyPlan {
            files: one_file_plan("hello\n", "hello world\n"),
            subject: "test: rewrite".to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(verify),
            on_committed: Box::new(move || {
                flag.set(true);
            }),
            context: (),
        };
        (plan, committed)
    }

    fn commit_count(vault: &Path) -> usize {
        git_lines(vault, &["rev-list", "--count", "HEAD"])
            .first()
            .and_then(|line| line.parse().ok())
            .unwrap_or(usize::MAX)
    }

    #[test]
    fn offline_run_writes_files_and_makes_one_commit() {
        let (_temp, vault, _guard) = scratch_vault("txn-ok");
        let child_env = ob::ChildEnv::new();
        let (plan, committed) = ok_hooks(|| Ok(()));
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        )
        .expect("offline run commits");
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello world\n"
        );
        assert_eq!(commit_count(&vault), 2);
        assert_eq!(outcome.subject, "test: rewrite");
        assert!(!outcome.sha.is_empty());
        assert_eq!(
            git_lines(&vault, &["log", "-1", "--format=%s"]),
            vec!["test: rewrite".to_string()]
        );
        assert_eq!(outcome.sync_pre, "skipped");
        assert_eq!(outcome.sync_post, "skipped");
        assert!(outcome.post_sync_warning.is_none());
        assert!(outcome.dirty_warning.is_none());
        assert!(committed.get(), "on_committed ran");
    }

    #[test]
    fn create_writes_a_missing_file_and_restore_removes_it() {
        let (_temp, vault, _guard) = scratch_vault("txn-create");
        let child_env = ob::ChildEnv::new();
        let created = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = std::rc::Rc::clone(&created);
        let plan = ReadyPlan {
            files: vec![FileRewrite {
                path: "2026/2026-W45.md".to_string(),
                before_contents: None,
                after_contents: "scaffold\n".to_string(),
                trash: false,
            }],
            subject: "test: create".to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(|| Ok(())),
            on_committed: Box::new(move || {
                flag.set(true);
            }),
            context: (),
        };
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        )
        .expect("create commits");
        assert_eq!(
            fs::read_to_string(vault.join("2026/2026-W45.md"))
                .expect("created"),
            "scaffold\n"
        );
        assert_eq!(outcome.subject, "test: create");
        assert!(created.get(), "on_committed ran");

        let plan = ReadyPlan {
            files: vec![FileRewrite {
                path: "2026/2026-W46.md".to_string(),
                before_contents: None,
                after_contents: "lost\n".to_string(),
                trash: false,
            }],
            subject: "test: create-fail".to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(|| Err("injected".to_string())),
            on_committed: Box::new(|| {}),
            context: (),
        };
        let failure = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        )
        .expect_err("verify failure rolls back a create");
        assert_eq!(failure.code, "verify_failed");
        assert!(
            !vault.join("2026/2026-W46.md").exists(),
            "failed create is removed on restore"
        );
    }

    #[test]
    fn create_is_stale_when_the_target_appears() {
        let (_temp, vault, _guard) = scratch_vault("txn-create-stale");
        let child_env = ob::ChildEnv::new();
        let target = vault.join("new.md");
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || {
                fs::write(&target, "concurrent\n").expect("concurrent create");
                Ok(ReadyPlan {
                    files: vec![FileRewrite {
                        path: "new.md".to_string(),
                        before_contents: None,
                        after_contents: "planned\n".to_string(),
                        trash: false,
                    }],
                    subject: "test: create-stale".to_string(),
                    post_write: Box::new(|| Ok(Vec::new())),
                    verify: Box::new(|| Ok(())),
                    on_committed: Box::new(|| {}),
                    context: (),
                })
            },
        );
        let failure = outcome.expect_err("appearing create is stale");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("new.md")).expect("kept"),
            "concurrent\n"
        );
        assert_eq!(commit_count(&vault), 1);
    }

    #[test]
    fn stale_note_aborts_before_writing() {
        let (_temp, vault, _guard) = scratch_vault("txn-stale");
        let child_env = ob::ChildEnv::new();
        let note = vault.join("note.md");
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || {
                // A concurrent writer lands between planning and the
                // snapshot/stale check.
                fs::write(&note, "concurrent edit\n").expect("concurrent");
                let (plan, _) = ok_hooks(|| Ok(()));
                Ok(ReadyPlan {
                    files: one_file_plan("hello\n", "hello world\n"),
                    ..plan
                })
            },
        );
        let failure = outcome.expect_err("stale aborts");
        assert_eq!(failure.code, "stale_note");
        assert!(failure.message.contains("note.md"), "{}", failure.message);
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap_or("")
                .contains("bob types rename"),
            "{:?}",
            failure.hint
        );
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "concurrent edit\n",
            "aborted run writes nothing"
        );
        assert_eq!(commit_count(&vault), 1, "aborted run commits nothing");
    }

    #[test]
    fn per_file_stale_rolls_back_earlier_writes() {
        let (_temp, vault, _guard) = scratch_vault("txn-per-file-stale");
        fs::write(vault.join("a.md"), "a before\n").expect("seed a");
        fs::write(vault.join("b.md"), "b before\n").expect("seed b");
        let before_images = snapshot_before_images(
            &vault,
            &["a.md".to_string(), "b.md".to_string()],
        );
        // Concurrent Obsidian save lands on B after the bulk check.
        fs::write(vault.join("b.md"), "b concurrent\n").expect("concurrent");
        let files = vec![
            FileRewrite {
                path: "a.md".to_string(),
                before_contents: Some("a before\n".to_string()),
                after_contents: "a after\n".to_string(),
                trash: false,
            },
            FileRewrite {
                path: "b.md".to_string(),
                before_contents: Some("b before\n".to_string()),
                after_contents: "b after\n".to_string(),
                trash: false,
            },
        ];
        let persisted = recovery::PersistedIntent {
            run_dir: vault.join(".trash-test-intent"),
            files: Vec::new(),
        };
        let failure = write_planned_files(
            &vault,
            &files,
            &before_images,
            &BTreeMap::new(),
            &persisted,
            COMMAND,
        )
        .expect_err("stale B aborts");
        assert_eq!(failure.code, "stale_note");
        assert!(failure.message.contains("b.md"), "{}", failure.message);
        let hint = failure.hint.as_deref().unwrap_or("");
        assert!(hint.contains("bob types rename"), "{hint:?}");
        assert!(!hint.contains('{'), "literal placeholder leaked: {hint:?}");
        assert_eq!(
            fs::read_to_string(vault.join("a.md")).expect("a"),
            "a before\n",
            "earlier write is rolled back"
        );
        assert_eq!(
            fs::read_to_string(vault.join("b.md")).expect("b"),
            "b concurrent\n",
            "concurrent bytes are kept"
        );
    }

    #[test]
    fn verify_failure_restores_before_images() {
        let (_temp, vault, _guard) = scratch_vault("txn-verify");
        let child_env = ob::ChildEnv::new();
        let (plan, committed) = ok_hooks(|| Err("boom".to_string()));
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        );
        let failure = outcome.expect_err("verify failure aborts");
        assert_eq!(failure.code, "verify_failed");
        assert!(failure.message.contains("boom"), "{}", failure.message);
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello\n",
            "verify failure rolls the file back"
        );
        assert_eq!(commit_count(&vault), 1, "verify failure commits nothing");
        assert!(!committed.get(), "on_committed never ran");
    }

    #[test]
    fn offline_dirty_paths_refuse() {
        let (_temp, vault, _guard) = scratch_vault("txn-dirty");
        fs::write(vault.join("note.md"), "hello\nuncommitted\n")
            .expect("dirty");
        let child_env = ob::ChildEnv::new();
        // The planner reads the dirty files, so before-images match the
        // dirty disk state and the run reaches the dirty-path refusal.
        let plan = ReadyPlan {
            files: one_file_plan("hello\nuncommitted\n", "hello world\n"),
            subject: "test: rewrite".to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(|| Ok(())),
            on_committed: Box::new(|| {}),
            context: (),
        };
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        );
        let failure = outcome.expect_err("dirty offline refuses");
        assert_eq!(failure.code, "dirty_paths");
        assert!(failure.message.contains("note.md"), "{}", failure.message);
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello\nuncommitted\n",
            "refused run writes nothing"
        );
        assert_eq!(commit_count(&vault), 1, "refused run commits nothing");
    }

    #[test]
    fn lock_busy_when_maintenance_lock_held() {
        let (temp, vault, _guard) = scratch_vault("txn-lock");
        // Hold the same lock file run() waits on: the guard above pinned
        // BOB_VAULT_SYNC_LOCK_FILE to it for this thread, and run() reads
        // that override in-process.
        let held = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(temp.path().join("txn-lock.lock"))
            .expect("open lock");
        fs2::FileExt::try_lock_exclusive(&held).expect("take lock");
        let child_env = ob::ChildEnv::new();
        let (plan, _) = ok_hooks(|| Ok(()));
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            true,
            move || Ok(plan),
        );
        let failure = outcome.expect_err("held lock refuses");
        assert_eq!(failure.code, "lock_busy");
        assert_eq!(commit_count(&vault), 1, "locked run commits nothing");
    }

    #[test]
    fn not_a_worktree_refuses() {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path().join("plain");
        fs::create_dir_all(&vault).expect("plain dir");
        let lock = temp.path().join("txn-plain.lock");
        let _guard = TestEnvGuard::set(&[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            Some(lock.as_os_str()),
        )]);
        let child_env = ob::ChildEnv::new();
        let (plan, _) = ok_hooks(|| Ok(()));
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        );
        let failure = outcome.expect_err("plain dir refuses");
        assert_eq!(failure.code, "not_a_worktree");
        assert!(
            failure.message.contains("theme renames"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn online_presync_failure_maps_to_sync_failed() {
        let (_temp, vault, _guard) = scratch_vault("txn-presync");
        // No origin remote: the pre-sync cycle fails fast with no network.
        let _sync = sync_env(&vault);
        let child_env = ob::ChildEnv::new();
        let (plan, _) = ok_hooks(|| Ok(()));
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            false,
            true,
            move || Ok(plan),
        );
        let failure = outcome.expect_err("pre-sync failure aborts");
        assert_eq!(failure.code, "sync_failed");
        assert!(
            failure.message.starts_with("pre-sync failed:"),
            "{}",
            failure.message
        );
        assert_eq!(commit_count(&vault), 1, "failed pre-sync commits nothing");
    }

    #[test]
    fn online_postsync_failure_reports_sync_pending() {
        let (_temp, vault, _guard) = scratch_vault("txn-postsync");
        // A local bare origin lets the pre-sync succeed with no network.
        let bare = _temp.path().join("origin.git");
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg("-q")
            .arg(&bare)
            .status()
            .expect("git init --bare runs");
        assert!(status.success());
        git(
            &vault,
            &["remote", "add", "origin", &bare.to_string_lossy()],
        );
        let _sync = sync_env(&vault);
        let child_env = ob::ChildEnv::new();
        // Break the remote after the pre-sync so only the post-sync fails;
        // the rename itself is still committed. The hook runs in-process on
        // this thread, so it uses the captured vault path (thread-local env
        // overrides never reach child processes).
        let hook_vault = vault.clone();
        let (plan, committed) = ok_hooks(move || {
            git(&hook_vault, &["remote", "remove", "origin"]);
            Ok(())
        });
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            false,
            true,
            move || Ok(plan),
        )
        .expect("post-sync failure still commits");
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello world\n"
        );
        assert_eq!(commit_count(&vault), 2, "rename committed");
        assert!(committed.get(), "on_committed ran");
        assert_eq!(outcome.sync_pre, "ok");
        assert_eq!(outcome.sync_post, "failed");
        let warning = outcome.post_sync_warning.expect("sync-pending warning");
        assert!(warning.contains("run bob vault-sync"), "{warning}");
    }

    /// Scratch vault with an isolated state dir, so intent journals never
    /// touch the real `~/.local/state`.
    fn scratch_vault_isolated(
        tag: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, PathBuf, TestEnvGuard) {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault dir");
        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "t@t"]);
        git(&vault, &["config", "user.name", "t"]);
        fs::write(vault.join("note.md"), "hello\n").expect("seed note");
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-qm", "init"]);
        let lock = temp.path().join(format!("{tag}.lock"));
        let state = tempfile::tempdir().expect("state dir");
        let guard = TestEnvGuard::set(&[
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
            ("XDG_STATE_HOME", Some(state.path().as_os_str())),
        ]);
        (temp, state, vault, guard)
    }

    fn trash_plan(path: &str, before: &str, subject: &str) -> ReadyPlan<()> {
        ReadyPlan {
            files: vec![FileRewrite {
                path: path.to_string(),
                before_contents: Some(before.to_string()),
                after_contents: String::new(),
                trash: true,
            }],
            subject: subject.to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(|| Ok(())),
            on_committed: Box::new(|| {}),
            context: (),
        }
    }

    fn seed_and_commit(vault: &Path, name: &str, contents: &str) {
        fs::write(vault.join(name), contents).expect("seed file");
        git(vault, &["add", "-A"]);
        git(vault, &["commit", "-qm", "seed"]);
    }

    #[test]
    fn trash_moves_file_and_commit_stages_deletion() {
        let (_temp, _state, vault, _guard) =
            scratch_vault_isolated("txn-trash");
        seed_and_commit(&vault, "old.md", "old\n");
        let child_env = ob::ChildEnv::new();
        let outcome = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(trash_plan("old.md", "old\n", "test: trash")),
        )
        .expect("trash commits");
        assert!(
            !vault.join("old.md").exists(),
            "trashed path is gone from the vault"
        );
        assert_eq!(
            fs::read_to_string(vault.join(".trash/old.md"))
                .expect("trash copy"),
            "old\n"
        );
        assert!(
            git_lines(
                &vault,
                &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD"]
            )
            .iter()
            .any(|line| line == "D\told.md"),
            "the commit stages the deletion"
        );
        assert!(outcome.recovery_warning.is_none());
        assert!(
            recover_pending(&vault, &child_env)
                .expect("no pending intent")
                .is_none(),
            "the intent is removed after the commit"
        );
    }

    #[test]
    fn trash_collision_uses_obsidian_suffix() {
        let (_temp, _state, vault, _guard) =
            scratch_vault_isolated("txn-trash-suffix");
        fs::create_dir_all(vault.join(".trash")).expect("trash dir");
        fs::write(vault.join(".trash/old.md"), "other\n").expect("squatter");
        seed_and_commit(&vault, "old.md", "old\n");
        let child_env = ob::ChildEnv::new();
        run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(trash_plan("old.md", "old\n", "test: trash")),
        )
        .expect("trash commits");
        assert!(!vault.join("old.md").exists());
        assert_eq!(
            fs::read_to_string(vault.join(".trash/old 2.md"))
                .expect("trash copy"),
            "old\n",
            "a colliding trash name gains an Obsidian suffix"
        );
        assert_eq!(
            fs::read_to_string(vault.join(".trash/old.md")).expect("squatter"),
            "other\n",
            "the colliding trash copy is left alone"
        );
    }

    #[test]
    fn trash_rolls_back_when_verify_fails() {
        let (_temp, _state, vault, _guard) =
            scratch_vault_isolated("txn-trash-rollback");
        seed_and_commit(&vault, "old.md", "old\n");
        let child_env = ob::ChildEnv::new();
        let plan = ReadyPlan {
            files: vec![
                FileRewrite {
                    path: "note.md".to_string(),
                    before_contents: Some("hello\n".to_string()),
                    after_contents: "hello world\n".to_string(),
                    trash: false,
                },
                FileRewrite {
                    path: "old.md".to_string(),
                    before_contents: Some("old\n".to_string()),
                    after_contents: String::new(),
                    trash: true,
                },
            ],
            subject: "test: trash-rollback".to_string(),
            post_write: Box::new(|| Ok(Vec::new())),
            verify: Box::new(|| Err("boom".to_string())),
            on_committed: Box::new(|| {}),
            context: (),
        };
        let failure = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || Ok(plan),
        )
        .expect_err("verify failure rolls back");
        assert_eq!(failure.code, "verify_failed");
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello\n",
            "the rewrite is restored"
        );
        assert_eq!(
            fs::read_to_string(vault.join("old.md")).expect("trashed"),
            "old\n",
            "the trashed file moves back"
        );
        assert!(
            !vault.join(".trash/old.md").exists(),
            "the trash copy moves back"
        );
        assert!(
            recover_pending(&vault, &child_env)
                .expect("no pending intent")
                .is_none(),
            "a rolled-back run leaves no intent"
        );
    }

    #[test]
    fn create_never_replaces_a_file_that_appeared() {
        let temp = tempfile::tempdir().expect("scratch dir");
        let target = temp.path().join("sub/new.md");
        install_create(&target, "hello\n").expect("first install wins");
        assert_eq!(fs::read_to_string(&target).expect("created"), "hello\n");
        let raced = install_create(&target, "other\n");
        assert!(
            matches!(raced, Err(CreateError::Appeared)),
            "a file that appeared after planning refuses: {raced:?}"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("kept"),
            "hello\n",
            "the raced file is never overwritten"
        );
        set_force_create_new(true);
        let fallback = temp.path().join("sub/fallback.md");
        install_create(&fallback, "fallback\n").expect("create_new installs");
        assert_eq!(
            fs::read_to_string(&fallback).expect("fallback"),
            "fallback\n"
        );
        let fallback_raced = install_create(&fallback, "other\n");
        assert!(
            matches!(fallback_raced, Err(CreateError::Appeared)),
            "the fallback also refuses: {fallback_raced:?}"
        );
        set_force_create_new(false);
    }

    #[test]
    fn crash_at_every_write_point_recovers_before_state() {
        for k in 0..=3 {
            let (_temp, _state, vault, _guard) =
                scratch_vault_isolated(&format!("txn-crash-{k}"));
            seed_and_commit(&vault, "a.md", "a before\n");
            seed_and_commit(&vault, "c.md", "c before\n");
            let child_env = ob::ChildEnv::new();
            set_crash_before_write(Some(k));
            let failure = run(
                &vault,
                &child_env,
                COMMAND,
                SUBJECT,
                true,
                false,
                move || {
                    Ok(ReadyPlan {
                        files: vec![
                            FileRewrite {
                                path: "a.md".to_string(),
                                before_contents: Some("a before\n".to_string()),
                                after_contents: "a after\n".to_string(),
                                trash: false,
                            },
                            FileRewrite {
                                path: "b.md".to_string(),
                                before_contents: None,
                                after_contents: "b after\n".to_string(),
                                trash: false,
                            },
                            FileRewrite {
                                path: "c.md".to_string(),
                                before_contents: Some("c before\n".to_string()),
                                after_contents: String::new(),
                                trash: true,
                            },
                        ],
                        subject: "test: crash".to_string(),
                        post_write: Box::new(|| Ok(Vec::new())),
                        verify: Box::new(|| Ok(())),
                        on_committed: Box::new(|| {}),
                        context: (),
                    })
                },
            )
            .expect_err("injected crash aborts");
            set_crash_before_write(None);
            assert_eq!(failure.code, "crash_injected", "k = {k}");
            let notice = recover_pending(&vault, &child_env)
                .expect("recovery runs")
                .expect("a crashed rewrite rolls back");
            assert!(notice.contains("rolled back"), "{notice}");
            assert_eq!(
                fs::read_to_string(vault.join("a.md")).expect("a"),
                "a before\n",
                "k = {k}: the rewrite is restored"
            );
            assert!(
                !vault.join("b.md").exists(),
                "k = {k}: the create is removed"
            );
            assert_eq!(
                fs::read_to_string(vault.join("c.md")).expect("c"),
                "c before\n",
                "k = {k}: the trashed file moves back"
            );
            assert!(
                !vault.join(".trash/c.md").exists(),
                "k = {k}: no trash copy remains"
            );
            assert_eq!(commit_count(&vault), 3, "k = {k}: nothing committed");
            assert!(
                recover_pending(&vault, &child_env)
                    .expect("intent cleared")
                    .is_none(),
                "k = {k}: recovery clears the intent"
            );
        }
    }

    #[test]
    fn file_edited_after_crash_refuses_recovery_pending() {
        let (_temp, _state, vault, _guard) =
            scratch_vault_isolated("txn-crash-diverged");
        seed_and_commit(&vault, "a.md", "a before\n");
        let child_env = ob::ChildEnv::new();
        set_crash_before_write(Some(2));
        let failure = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || {
                Ok(ReadyPlan {
                    files: vec![
                        FileRewrite {
                            path: "a.md".to_string(),
                            before_contents: Some("a before\n".to_string()),
                            after_contents: "a after\n".to_string(),
                            trash: false,
                        },
                        FileRewrite {
                            path: "b.md".to_string(),
                            before_contents: None,
                            after_contents: "b after\n".to_string(),
                            trash: false,
                        },
                    ],
                    subject: "test: crash-diverged".to_string(),
                    post_write: Box::new(|| Ok(Vec::new())),
                    verify: Box::new(|| Ok(())),
                    on_committed: Box::new(|| {}),
                    context: (),
                })
            },
        )
        .expect_err("injected crash aborts");
        set_crash_before_write(None);
        assert_eq!(failure.code, "crash_injected");
        // An editor saves over the crashed after-image: neither image matches.
        fs::write(vault.join("a.md"), "tampered\n").expect("tamper");
        let failure = recover_pending(&vault, &child_env)
            .expect_err("diverged recovery refuses");
        assert_eq!(failure.code, "recovery_pending");
        assert!(failure.message.contains("a.md"), "{}", failure.message);
        assert!(failure.message.contains("manifest"), "{}", failure.message);
        assert!(
            failure.hint.as_deref().unwrap_or("").contains("vault-sync"),
            "{:?}",
            failure.hint
        );
        assert_eq!(
            fs::read_to_string(vault.join("a.md")).expect("tampered"),
            "tampered\n",
            "refused recovery touches nothing"
        );
        assert_eq!(
            fs::read_to_string(vault.join("b.md")).expect("created"),
            "b after\n",
            "refused recovery touches nothing"
        );
    }

    #[test]
    fn crash_after_commit_only_removes_intent() {
        let (_temp, _state, vault, _guard) =
            scratch_vault_isolated("txn-crash-committed");
        let child_env = ob::ChildEnv::new();
        set_crash_after_commit(true);
        let failure = run(
            &vault,
            &child_env,
            COMMAND,
            SUBJECT,
            true,
            false,
            move || {
                let (plan, _) = ok_hooks(|| Ok(()));
                Ok(plan)
            },
        )
        .expect_err("injected crash aborts");
        assert_eq!(failure.code, "crash_injected");
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello world\n",
            "the committed write stays"
        );
        assert_eq!(commit_count(&vault), 2, "the commit landed");
        assert!(
            recover_pending(&vault, &child_env)
                .expect("recovery runs")
                .is_none(),
            "a crash after the commit only removes the intent"
        );
        assert_eq!(
            fs::read_to_string(vault.join("note.md")).expect("note"),
            "hello world\n",
            "recovery changes nothing"
        );
        assert_eq!(commit_count(&vault), 2, "recovery commits nothing");
    }
}
