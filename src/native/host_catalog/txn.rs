//! The host-catalog transaction: guarded install plus one scoped commit.
//!
//! [`apply_and_commit`] is the lock-free core. It never takes the
//! maintenance lock and never runs a vault-sync cycle, so nightly (which
//! already holds the lock) and a later capture transaction can compose it.
//! [`run_locked`] wraps it for standalone commands with the same lifecycle
//! as `vault_rewrite::run`: vault identity, bounded lock wait, worktree
//! check, pre-sync, plan, guarded write, scoped commit, post-sync. It does
//! not copy that module's rename/journal assumptions: installs go through the
//! shared capture staged-file writer (disk-preimage checks, no-replace
//! creation, rollback), and recovery never restores over a later edit.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use super::FileEdit;
use crate::native::{
    capture::{
        changed_after_planning, read_failed, write_staged_files_guarded,
        CaptureError, CaptureFailureKind, StagedTextFile,
    },
    collect_done,
    ob::{self, ChildEnv, LockWaitError},
    style::Styler,
    vault_rewrite::{
        committed_despite_error, git_head, unstage_paths, TxnFailure,
    },
    vault_sync,
};

/// Bounded wait for `bob_sync.lock` before a standalone write.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// Files a plan read but does not rewrite, with the exact bytes it saw
/// (`None` means the path must still be absent). Validated at both guard
/// points of the shared writer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReadDependencies {
    files: BTreeMap<PathBuf, Option<String>>,
}

impl ReadDependencies {
    pub(crate) fn insert(&mut self, path: PathBuf, expected: Option<String>) {
        self.files.insert(path, expected);
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (&Path, Option<&str>)> + '_ {
        self.files
            .iter()
            .map(|(path, contents)| (path.as_path(), contents.as_deref()))
    }

    pub(crate) fn validate(&self, vault: &Path) -> Result<(), CaptureError> {
        for (relative, expected) in &self.files {
            let absolute = vault.join(relative);
            match (fs::read(&absolute), expected) {
                (Ok(bytes), Some(expected)) if bytes == expected.as_bytes() => {
                }
                (Ok(_), Some(_)) => {
                    return Err(changed_after_planning(format!(
                        "{} changed on disk after planning",
                        relative.display()
                    )));
                }
                (Ok(_), None) => {
                    return Err(changed_after_planning(format!(
                        "{} appeared on disk after planning",
                        relative.display()
                    )));
                }
                (Err(error), Some(_))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(changed_after_planning(format!(
                        "{} was deleted after planning",
                        relative.display()
                    )));
                }
                (Err(error), None)
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                (Err(error), _) => {
                    return Err(read_failed(format!(
                        "read {}: {error}",
                        absolute.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// One planned vault write: staged files, the read inputs guarded beside
/// them, the single scoped commit subject, and a post-install verifier.
pub(crate) struct PlannedTxn<T> {
    pub(crate) files: Vec<StagedTextFile>,
    pub(crate) deps: ReadDependencies,
    pub(crate) subject: String,
    pub(crate) verify: Box<dyn FnOnce() -> Result<(), String>>,
    pub(crate) context: T,
    /// Noun used in install/verify error messages, e.g. `"notebook"`.
    pub(crate) kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitInfo {
    pub(crate) sha: String,
    pub(crate) subject: String,
}

/// What a finished transaction did. `commit` is `None` for a no-op plan.
#[derive(Debug)]
pub(crate) struct TxnResult<T> {
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: &'static str,
    pub(crate) sync_post: &'static str,
    pub(crate) post_sync_warning: Option<String>,
    pub(crate) context: T,
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    hint: &str,
) -> TxnFailure {
    TxnFailure::new(code, message, Some(hint.to_string()))
}

fn relative_of(vault: &Path, target: &Path) -> PathBuf {
    target.strip_prefix(vault).unwrap_or(target).to_path_buf()
}

/// Stage each host rewrite as an in-place update of an existing file.
pub(crate) fn staged_rewrites<'a>(
    vault: &Path,
    edits: impl IntoIterator<Item = &'a FileEdit>,
) -> Vec<StagedTextFile> {
    edits
        .into_iter()
        .map(|edit| StagedTextFile {
            target: vault.join(&edit.path),
            target_existed: true,
            original_target: edit.before.clone(),
            updated_target: edit.after.clone(),
        })
        .collect()
}

/// Restore each staged file's before-image only while it still holds our
/// after-image; a later edit is never overwritten. Returns the paths left
/// in place because they diverged or could not be restored.
fn rollback_guarded(files: &[StagedTextFile]) -> Vec<String> {
    let mut left = Vec::new();
    for staged in files {
        let current = fs::read(&staged.target).ok();
        if current.as_deref() != Some(staged.updated_target.as_bytes()) {
            if current.is_some() || staged.target_existed {
                left.push(format!(
                    "{} changed since the write and was left in place",
                    staged.target.display()
                ));
            }
            continue;
        }
        let result = if staged.target_existed {
            collect_done::atomic_write(&staged.target, &staged.original_target)
        } else {
            fs::remove_file(&staged.target)
        };
        if let Err(error) = result {
            left.push(format!(
                "could not restore {}: {error}",
                staged.target.display()
            ));
        }
    }
    left
}

fn rollback_note(left: &[String]) -> String {
    if left.is_empty() {
        "; the write was rolled back".to_string()
    } else {
        format!("; rollback issues: {}", left.join("; "))
    }
}

/// Install, verify, and commit one plan. No lock, no vault-sync cycles.
pub(crate) fn apply_and_commit<T>(
    vault: &Path,
    child_env: &ChildEnv,
    planned: PlannedTxn<T>,
) -> Result<(Option<CommitInfo>, T), TxnFailure> {
    let PlannedTxn {
        files,
        deps,
        subject,
        verify,
        context,
        kind,
    } = planned;
    if files.is_empty() {
        return Ok((None, context));
    }
    let relatives: Vec<PathBuf> = files
        .iter()
        .map(|staged| relative_of(vault, &staged.target))
        .collect();
    let old_head = git_head(vault, child_env).ok();

    if let Err(error) =
        write_staged_files_guarded(&files, || deps.validate(vault))
    {
        let stale = error.typed == Some(CaptureFailureKind::NoteChanged);
        return Err(if stale {
            failure(
                "stale_note",
                error.message,
                "re-run the command to replan from the latest files",
            )
        } else {
            failure(
                "verify_failed",
                format!(
                    "could not install the {kind} files: {}",
                    error.message
                ),
                "the write was rolled back; inspect the listed paths and retry",
            )
        });
    }

    if let Err(message) = verify() {
        let left = rollback_guarded(&files);
        return Err(failure(
            "verify_failed",
            format!(
                "could not verify the written {kind} files: {message}{}",
                rollback_note(&left)
            ),
            "the write was rolled back; inspect the listed paths and retry",
        ));
    }

    let rolled_back = |unstage: String, left: Vec<String>| {
        format!("{unstage}{}", rollback_note(&left))
    };
    let sha = match ob::commit_paths(vault, child_env, &subject, &relatives) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            let unstage = unstage_paths(vault, child_env, &relatives);
            let left = rollback_guarded(&files);
            return Err(failure(
                "verify_failed",
                format!(
                    "the write produced no scoped Git commit{}",
                    rolled_back(unstage, left)
                ),
                "check the vault Git state and retry",
            ));
        }
        Err(error) => {
            match committed_despite_error(
                vault,
                child_env,
                old_head.as_deref(),
                &subject,
            ) {
                Some((sha, committed)) if committed == subject => sha,
                _ => {
                    let unstage = unstage_paths(vault, child_env, &relatives);
                    let left = rollback_guarded(&files);
                    return Err(failure(
                        "verify_failed",
                        format!(
                            "Git commit failed: {error}{}",
                            rolled_back(unstage, left)
                        ),
                        "check the vault Git state and retry",
                    ));
                }
            }
        }
    };
    Ok((Some(CommitInfo { sha, subject }), context))
}

fn sync_cycle(child_env: &ChildEnv) -> Result<(), String> {
    let cycle =
        vault_sync::run_cycle_with_existing_lock_report(child_env, true);
    if cycle.ok && cycle.conflicts.is_empty() {
        return Ok(());
    }
    Err(if cycle.conflicts.is_empty() {
        cycle
            .error
            .unwrap_or_else(|| "vault sync reported failure".to_string())
    } else {
        format!("vault sync conflict in {}", cycle.conflicts.join(", "))
    })
}

/// Standalone lifecycle around [`apply_and_commit`]. `plan` runs after the
/// pre-sync, so it sees the synced snapshot. A no-op plan writes nothing,
/// commits nothing, and skips the post-sync. `kind` is the noun in the
/// worktree error (`"{kind} writes require a Git worktree"`).
pub(crate) fn run_locked<T>(
    vault: &Path,
    child_env: &ChildEnv,
    json_mode: bool,
    kind: &'static str,
    plan: impl FnOnce() -> Result<PlannedTxn<T>, TxnFailure>,
) -> Result<TxnResult<T>, TxnFailure> {
    if let Some(detail) = vault_sync::vault_identity_problem(vault) {
        return Err(failure(
            "vault_mismatch",
            detail,
            "point BOB_DIR at the vault passed with -b",
        ));
    }
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
                let message = match error {
                    LockWaitError::Timeout { path } => format!(
                        "another Bob vault maintenance run is already active (waited on {})",
                        path.display()
                    ),
                    other => format!(
                        "could not acquire the vault maintenance lock: {other}"
                    ),
                };
                return Err(failure(
                    "lock_busy",
                    message,
                    "wait for the other maintenance run to finish, then retry",
                ));
            }
        }
    };
    match ob::detect_git_worktree(vault, child_env) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return Err(failure(
                "not_a_worktree",
                format!(
                    "{kind} writes require a Git worktree: {}",
                    vault.display()
                ),
                "initialize or open the Bob vault as a Git worktree",
            ));
        }
    }
    if let Err(detail) = sync_cycle(child_env) {
        return Err(failure(
            "sync_failed",
            format!("pre-sync failed: {detail}"),
            "run bob vault-sync, resolve the problem, then retry",
        ));
    }

    let planned = plan()?;
    let (commit, context) = apply_and_commit(vault, child_env, planned)?;
    let (sync_post, post_sync_warning) = if commit.is_none() {
        ("skipped", None)
    } else {
        match sync_cycle(child_env) {
            Ok(()) => ("ok", None),
            Err(detail) => (
                "failed",
                Some(format!(
                    "the change is committed locally; post-sync failed ({detail}); run bob vault-sync"
                )),
            ),
        }
    };
    Ok(TxnResult {
        commit,
        sync_pre: "ok",
        sync_post,
        post_sync_warning,
        context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "t@t"]);
        git(&vault, &["config", "user.name", "t"]);
        fs::write(vault.join("host.md"), "host\n").unwrap();
        fs::write(vault.join("ancestor.md"), "ancestor\n").unwrap();
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-qm", "init"]);
        (temp, vault)
    }

    fn staged(vault: &Path, before: &str, after: &str) -> Vec<StagedTextFile> {
        vec![
            StagedTextFile {
                target: vault.join("host.md"),
                target_existed: true,
                original_target: before.to_string(),
                updated_target: after.to_string(),
            },
            StagedTextFile {
                target: vault.join("new_note.md"),
                target_existed: false,
                original_target: String::new(),
                updated_target: "fresh\n".to_string(),
            },
        ]
    }

    fn plan(
        vault: &Path,
        files: Vec<StagedTextFile>,
        deps: ReadDependencies,
    ) -> PlannedTxn<()> {
        let _ = vault;
        PlannedTxn {
            files,
            deps,
            subject: "notebooks: test".to_string(),
            verify: Box::new(|| Ok(())),
            context: (),
            kind: "notebook",
        }
    }

    fn deps(contents: &str) -> ReadDependencies {
        let mut deps = ReadDependencies::default();
        deps.insert(PathBuf::from("ancestor.md"), Some(contents.to_string()));
        deps
    }

    fn head(vault: &Path) -> String {
        git(vault, &["rev-parse", "HEAD"])
    }

    #[test]
    fn commits_only_the_staged_paths() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("unrelated.md"), "dirty\n").unwrap();
        let (commit, ()) = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect("applies");
        let commit = commit.expect("committed");
        assert_eq!(commit.subject, "notebooks: test");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host+\n"
        );
        assert_eq!(
            fs::read_to_string(vault.join("new_note.md")).unwrap(),
            "fresh\n"
        );
        let changed =
            git(&vault, &["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(
            changed.lines().collect::<Vec<_>>(),
            ["host.md", "new_note.md"]
        );
        assert!(
            git(&vault, &["status", "--porcelain"]).contains("unrelated.md")
        );
    }

    #[test]
    fn empty_plan_is_a_no_op() {
        let (_temp, vault) = scratch();
        let before = head(&vault);
        let (commit, ()) = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(&vault, Vec::new(), ReadDependencies::default()),
        )
        .expect("no-op");
        assert!(commit.is_none());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn stale_read_dependency_refuses_before_any_write() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("ancestor.md"), "edited meanwhile\n").unwrap();
        let before = head(&vault);
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("stale");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn stale_preimage_and_planted_destination_refuse_without_overwriting() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("host.md"), "foreign edit\n").unwrap();
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("stale preimage");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "foreign edit\n"
        );

        fs::write(vault.join("host.md"), "host\n").unwrap();
        fs::write(vault.join("new_note.md"), "someone else's file\n").unwrap();
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("destination appeared");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("new_note.md")).unwrap(),
            "someone else's file\n"
        );
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn commit_failure_rolls_back_the_write_and_unstages() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, vault) = scratch();
        let hook = vault.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let before = head(&vault);
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("commit fails");
        assert_eq!(failure.code, "verify_failed");
        assert!(
            failure.message.contains("Git commit failed"),
            "{}",
            failure.message
        );
        assert!(
            failure.message.contains("rolled back"),
            "{}",
            failure.message
        );
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
        assert_eq!(git(&vault, &["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn verify_failure_rolls_back_without_committing() {
        let (_temp, vault) = scratch();
        let before = head(&vault);
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.verify = Box::new(|| Err("still stale".to_string()));
        let failure =
            apply_and_commit(&vault, &Vec::new(), planned).expect_err("verify");
        assert_eq!(failure.code, "verify_failed");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn rollback_never_overwrites_a_later_edit() {
        let (_temp, vault) = scratch();
        let files = staged(&vault, "host\n", "host+\n");
        fs::write(vault.join("host.md"), "host+\n").unwrap();
        fs::write(vault.join("new_note.md"), "fresh\n").unwrap();
        // The host changed again after our write; the created file is intact.
        fs::write(vault.join("host.md"), "later foreign edit\n").unwrap();
        let left = rollback_guarded(&files);
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].contains("host.md"));
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "later foreign edit\n"
        );
        assert!(
            !vault.join("new_note.md").exists(),
            "our own file is removed"
        );
    }
}
