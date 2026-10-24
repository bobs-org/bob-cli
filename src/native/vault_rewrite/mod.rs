//! One shared vault-rewrite transaction for global renames.
//!
//! Extracted from `bob themes rename` (`src/native/pomodoro_themes/rename.rs`)
//! so every vocabulary rename — themes today, relation labels next — runs the
//! same steps: bounded lock wait (`lock_busy`), the worktree check
//! (`not_a_worktree`), pre-sync unless offline (`sync_failed`), planning
//! through a caller closure over the synced files, before-image snapshots and
//! the stale check (`stale_note`), dirty-path rules (offline refuses with
//! `dirty_paths`; online notes pre-sync inclusion), atomic writes with
//! rollback, a post-write hook for the journal append, a verify hook that
//! restores on failure (`verify_failed`), one commit with the caller's
//! subject, unstaging on failure, and committed-despite-error handling, then
//! post-sync, with "sync pending" reported separately.
//!
//! The caller owns everything domain-specific: planning and validation,
//! the journal event, verification, the success report, and all output. The
//! failure codes and message shapes below are the `bob themes rename`
//! contract, kept so that move stays byte-for-byte.

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
    ref_library::migrate_tasks,
    style::Styler,
    vault_sync,
};

/// Bounded wait for `bob_sync.lock` before any vault rewrite.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// One transaction failure: a stable machine-readable code, the human
/// message, and an optional retry hint. Codes are the rename contract
/// (`lock_busy`, `not_a_worktree`, `sync_failed`, `stale_note`,
/// `dirty_paths`, `verify_failed`, plus `io` for the Git status read).
#[derive(Debug, Clone)]
pub(crate) struct TxnFailure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
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
        }
    }
}

/// One planned file rewrite: vault-relative `/`-joined path plus the exact
/// before-image the planner saw and the after-image to write. The stale
/// check compares the live file against `before_contents` right before
/// writing.
#[derive(Debug, Clone)]
pub(crate) struct FileRewrite {
    pub(crate) path: String,
    pub(crate) before_contents: String,
    pub(crate) after_contents: String,
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
/// dirty-inclusion warning, the optional post-sync-failure warning, and
/// the caller's untouched context.
#[derive(Debug)]
pub(crate) struct TxnOutcome<C> {
    pub(crate) sha: String,
    pub(crate) subject: String,
    pub(crate) sync_pre: &'static str,
    pub(crate) sync_post: &'static str,
    pub(crate) post_sync_warning: Option<String>,
    pub(crate) dirty_warning: Option<String>,
    pub(crate) context: C,
}

/// Run one vault-rewrite transaction: lock, worktree check, pre-sync, plan,
/// snapshot and stale check, dirty rules, atomic writes, post-write hook,
/// verify hook, one commit, post-commit hook, and post-sync.
pub(crate) fn run<C>(
    vault: &Path,
    child_env: &ob::ChildEnv,
    command_name: &'static str,
    worktree_subject: &str,
    offline: bool,
    json_mode: bool,
    plan: impl FnOnce() -> Result<ReadyPlan<C>, TxnFailure>,
) -> Result<TxnOutcome<C>, TxnFailure> {
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

    let ready = plan()?;
    let touched_relative: BTreeSet<PathBuf> = ready
        .files
        .iter()
        .map(|file| PathBuf::from(&file.path))
        .collect();

    // Snapshot each target and compare again right before writing.
    let relatives: Vec<String> =
        ready.files.iter().map(|file| file.path.clone()).collect();
    let before_images = snapshot_before_images(vault, &relatives);
    // Re-read to detect a change since planning.
    for file in &ready.files {
        let absolute = vault.join(&file.path);
        let current = fs::read_to_string(&absolute).unwrap_or_default();
        if current != file.before_contents {
            let _ =
                restore_before_images(vault, &before_images, &BTreeMap::new());
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
    // Write each file atomically, then run the post-write hook.
    for file in &ready.files {
        let absolute = vault.join(&file.path);
        if let Err(error) =
            collect_done::atomic_write(&absolute, &file.after_contents)
        {
            let _ =
                restore_before_images(vault, &before_images, &BTreeMap::new());
            return Err(TxnFailure::new(
                "verify_failed",
                format!("could not write {}: {error}", file.path),
                Some("the rename was rolled back; inspect the listed paths and retry"),
            ));
        }
    }
    let extra_paths = match (ready.post_write)() {
        Ok(paths) => paths,
        Err(message) => {
            let _ = restore_before_images(
                vault,
                &before_images,
                &after_images(vault, &ready.files),
            );
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
        );
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
            );
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
                    );
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
                );
                return Err(TxnFailure::new(
                    "verify_failed",
                    format!("Git commit failed: {error}{unstage}"),
                    Some("check the vault Git state and retry"),
                ));
            }
        }
    };

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
        context: ready.context,
    })
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
        map.insert(
            vault.join(&file.path),
            file.after_contents.as_bytes().to_vec(),
        );
    }
    map
}

fn restore_before_images(
    vault: &Path,
    before: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> String {
    let rels = before
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
    for (absolute, bytes) in before {
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

fn git_head(vault: &Path, child_env: &ob::ChildEnv) -> Result<String, String> {
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

fn unstage_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: &[PathBuf],
) -> String {
    let output = ob::git_command(vault, child_env)
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

fn committed_despite_error(
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

    const COMMAND: &str = "bob themes rename";
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
        let guard = TestEnvGuard::set(&[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            Some(lock.as_os_str()),
        )]);
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

    fn one_file_plan(before: &str, after: &str) -> Vec<FileRewrite> {
        vec![FileRewrite {
            path: "note.md".to_string(),
            before_contents: before.to_string(),
            after_contents: after.to_string(),
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
                .contains("bob themes rename"),
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
}
