//! Vault-backed off-clock classification for the plan budget.
//!
//! The budget finds candidate links by syntax alone, then classifies each
//! distinct `(target, block_id)` pair once with the same link-clock
//! resolution the session clock rule uses. Unresolved, ambiguous,
//! unreadable, or not-yet-cached targets stay counted.

use std::path::{Path, PathBuf};

use super::{compute_for_daily_with, LedgerBudget};
use crate::native::{
    capture_pomodoro_close::CloseVault,
    capture_pomodoro_start::LinkClockContext,
    config::PlanConfig,
    vault_links::{LinkResolution, VaultLinkResolver},
};

/// Read-only disk vault for budget callers. Resolves like
/// `SnapshotCloseVault` without staging and normalizes found paths to
/// absolute.
pub(crate) struct DiskCloseVault {
    bob_dir: PathBuf,
    resolver: VaultLinkResolver,
}

impl DiskCloseVault {
    pub(crate) fn new(bob_dir: PathBuf) -> Self {
        Self {
            resolver: VaultLinkResolver::new(bob_dir.clone()),
            bob_dir,
        }
    }
}

impl CloseVault for DiskCloseVault {
    fn bob_dir(&self) -> &Path {
        &self.bob_dir
    }

    fn resolve_target(&self, from_path: &Path, target: &str) -> LinkResolution {
        if target.is_empty() {
            return LinkResolution::Found(from_path.to_path_buf());
        }
        match self.resolver.resolve(from_path, target) {
            LinkResolution::Found(path) => {
                if path.is_absolute() {
                    LinkResolution::Found(path)
                } else {
                    LinkResolution::Found(self.bob_dir.join(path))
                }
            }
            other => other,
        }
    }

    fn read_latest(&self, path: &Path) -> Result<Option<String>, String> {
        let filesystem_path =
            if path.is_absolute() || path.starts_with(&self.bob_dir) {
                path.to_path_buf()
            } else {
                self.bob_dir.join(path)
            };
        if !filesystem_path.is_file() {
            return Ok(None);
        }
        std::fs::read_to_string(&filesystem_path)
            .map(Some)
            .map_err(|error| {
                format!("cannot read {}: {error}", filesystem_path.display())
            })
    }
}

/// Vault-aware budget: off-clock pairs leave `links.count`, `status`, and
/// the cap lint, and are reported in `links.off_clock`.
pub(crate) fn compute_for_daily_in_vault<V: CloseVault + ?Sized>(
    vault: &V,
    day_relative: &Path,
    contents: &str,
    config: &PlanConfig,
    daily_key: Option<&str>,
) -> LedgerBudget {
    let mut context =
        LinkClockContext::new(Some(vault), day_relative, contents);
    let daily_canonical =
        daily_key.map(|key| key.strip_suffix(".md").unwrap_or(key).to_string());
    compute_for_daily_with(
        contents,
        config,
        daily_key,
        &mut |target, block_id| {
            let path_part = match &daily_canonical {
                Some(daily) if target == daily => "",
                _ => target,
            };
            let fact = context.resolve(path_part, block_id, false);
            fact.fact.off_clock == Some(true)
        },
    )
}
