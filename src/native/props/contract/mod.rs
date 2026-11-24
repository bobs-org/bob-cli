//! Area/project setting definitions and validation shared by every props writer.

pub(crate) mod expand;
pub(crate) mod parent;
pub(crate) mod settings;
pub(crate) mod validate;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::native::{
    capture_targets::{scan_capture_targets, CaptureTarget},
    config::themes::{load_theme_sources, ThemeSources},
    notebooks::snapshot::VaultSnapshot,
};

use super::PropsError;

/// Read-only dependencies, refreshed along with the note under the write lock.
#[derive(Debug, Clone)]
pub(crate) struct Context {
    pub(crate) snapshot: VaultSnapshot,
    pub(crate) targets: Vec<CaptureTarget>,
    pub(crate) types: ThemeSources,
}

impl Context {
    pub(crate) fn load(vault: &Path) -> Self {
        Self {
            snapshot: VaultSnapshot::load(vault),
            targets: scan_capture_targets(vault).targets,
            types: load_theme_sources(),
        }
    }
}

pub(crate) fn error(
    code: &str,
    key: &str,
    index: usize,
    message: impl Into<String>,
    hint: &str,
) -> PropsError {
    PropsError {
        code: code.to_owned(),
        key: Some(key.to_owned()),
        op_index: Some(index),
        line: None,
        message: message.into(),
        hint: hint.to_owned(),
    }
}
