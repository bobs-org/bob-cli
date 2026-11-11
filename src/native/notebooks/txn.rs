//! The notebook transaction: guarded install plus one scoped commit.
//!
//! The lock-free core and standalone lifecycle live in
//! [`crate::native::host_catalog::txn`]. This module keeps the notebook
//! snapshot helpers and the post-install host verifier.

use std::path::{Path, PathBuf};

use super::{scan::analyze, snapshot::VaultSnapshot};
use crate::native::{host_catalog, ob::ChildEnv, vault_rewrite::TxnFailure};

pub(crate) use crate::native::host_catalog::{
    apply_and_commit, CommitInfo, PlannedTxn, ReadDependencies, TxnResult,
};

impl ReadDependencies {
    /// Depend on every retained note of `snapshot` (notebooks, areas, and
    /// projects: all the bytes a catalog plan reads).
    pub(crate) fn from_snapshot(snapshot: &VaultSnapshot) -> Self {
        let mut deps = Self::default();
        for (path, file) in &snapshot.files {
            if let Some(contents) = &file.contents {
                deps.insert(path.clone(), Some(contents.clone()));
            }
        }
        deps
    }

    /// Depend on the retained notes in `paths` only.
    pub(crate) fn from_paths<'a>(
        snapshot: &VaultSnapshot,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Self {
        let mut deps = Self::default();
        for path in paths {
            if let Some(contents) = snapshot.contents(path) {
                deps.insert(path.to_path_buf(), Some(contents.to_string()));
            }
        }
        deps
    }
}

/// Re-read the vault and report any affected host that is not `current`,
/// as a post-install verification.
pub(crate) fn verify_hosts_current(
    vault: PathBuf,
    hosts: Vec<PathBuf>,
) -> Box<dyn FnOnce() -> Result<(), String>> {
    Box::new(move || {
        let snapshot = VaultSnapshot::load(&vault);
        if let Some(error) = snapshot.io_errors.first() {
            return Err(error.clone());
        }
        let analysis = analyze(&snapshot);
        for host in hosts {
            let key = host.to_string_lossy().replace('\\', "/");
            let still_stale = analysis
                .plan
                .hosts
                .iter()
                .any(|plan| plan.record.path == key && plan.edit.is_some());
            if still_stale {
                return Err(format!(
                    "{key} is still not current after the write"
                ));
            }
        }
        Ok(())
    })
}

/// Standalone lifecycle around [`apply_and_commit`] for notebook writes.
pub(crate) fn run_locked<T>(
    vault: &Path,
    child_env: &ChildEnv,
    json_mode: bool,
    plan: impl FnOnce() -> Result<PlannedTxn<T>, TxnFailure>,
) -> Result<TxnResult<T>, TxnFailure> {
    host_catalog::run_locked(vault, child_env, json_mode, "notebook", plan)
}
