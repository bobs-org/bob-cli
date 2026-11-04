//! Shared maintenance shell for task rewrite commands.
//!
//! The planner owns the transformation; this module owns the common lock,
//! sync, scoped-commit, and guarded-write lifecycle used by task move and split.

use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    ob::{self, ChildEnv},
    vault_sync::{self, CycleReport},
};

pub(crate) struct TaskCommandShell {
    _lock: File,
    pub(crate) child_env: ChildEnv,
    pub(crate) worktree: bool,
    pub(crate) warning: Option<String>,
    pub(crate) pre_sync: Option<CycleReport>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommitResult {
    pub(crate) sha: Option<String>,
    pub(crate) subject: String,
    pub(crate) paths: Vec<PathBuf>,
}

impl TaskCommandShell {
    pub(crate) fn start(
        vault: &Path,
        retry_timeout: Duration,
        offline: bool,
        json_mode: bool,
    ) -> Result<Self, String> {
        let lock = ob::acquire_lock_waiting(retry_timeout, || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!("  waiting for another vault maintenance run…");
            }
        })
        .map_err(|error| format!("acquire vault maintenance lock: {error}"))?;
        let child_env = ob::child_env();
        let (worktree, warning) = match ob::detect_git_worktree(vault, &child_env) {
            Ok(true) => (true, None),
            Ok(false) => (
                false,
                Some(format!(
                    "vault is not a git worktree ({}); skipping commit and sync",
                    vault.display()
                )),
            ),
            Err(error) => (
                false,
                Some(format!("git is unavailable ({error}); skipping commit and sync")),
            ),
        };
        let pre_sync = if worktree && !offline {
            Some(vault_sync::run_cycle_with_existing_lock_report(
                &child_env, true,
            ))
        } else {
            None
        };
        Ok(Self {
            _lock: lock,
            child_env,
            worktree,
            warning,
            pre_sync,
        })
    }

    pub(crate) fn commit(
        &self,
        vault: &Path,
        applied: &[PathBuf],
        subject: &str,
    ) -> Result<Option<CommitResult>, String> {
        commit_paths(vault, &self.child_env, applied, subject)
    }

    pub(crate) fn post_sync(&self) -> CycleReport {
        run_post_sync(&self.child_env)
    }
}

pub(crate) fn commit_paths(
    vault: &Path,
    child_env: &ChildEnv,
    applied: &[PathBuf],
    subject: &str,
) -> Result<Option<CommitResult>, String> {
    if applied.is_empty() {
        return Ok(None);
    }
    let relative = applied
        .iter()
        .map(|path| {
            path.strip_prefix(vault)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone())
        })
        .collect::<Vec<_>>();
    let sha = ob::commit_paths(vault, child_env, subject, &relative)?;
    Ok(Some(CommitResult {
        sha,
        subject: subject.to_string(),
        paths: relative,
    }))
}

pub(crate) fn run_post_sync(child_env: &ChildEnv) -> CycleReport {
    vault_sync::run_cycle_with_existing_lock_report(child_env, true)
}
