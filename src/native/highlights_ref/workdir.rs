//! Private scratch staging for Highlights PDF routes.
//!
//! Renders, downloads, stamped copies, and episode audio live in a `0700`
//! scratch directory until the final atomic installs. This is because
//! `scan` and `bob_xlib_pull` read every file in `xlib/`, so a listen run
//! lasting minutes must never stage inside the vault.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{CommandError, Result};

/// Keep the scratch directory for debugging.
pub(super) const ENV_KEEP_WORKDIR: &str = "BOB_HIGHLIGHTS_KEEP_WORKDIR";
/// Legacy keep variable from the clip-only era.
const ENV_LEGACY_KEEP_WORKDIR: &str = "BOB_WEB_CLIP_KEEP_WORKDIR";

/// A `0700` scratch directory, removed on drop unless kept for debugging.
pub(super) struct ScratchDir {
    path: PathBuf,
    keep: bool,
}

impl ScratchDir {
    /// Create a scratch directory tagged with `prefix`
    /// (`bob-<prefix>-<pid>-<nanos>`).
    pub(super) fn create(prefix: &str) -> Result<Self> {
        let base = env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|base| {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    base.as_os_str().as_bytes().len() <= 40
                }
                #[cfg(not(unix))]
                {
                    base.as_os_str().len() <= 40
                }
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = base.join(format!("bob-{prefix}-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).map_err(|error| {
            CommandError::new(format!(
                "create workdir {}: {error}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|error| {
                    CommandError::new(format!(
                        "secure workdir {}: {error}",
                        path.display()
                    ))
                })?;
        }
        let keep = env::var_os(ENV_KEEP_WORKDIR).as_deref()
            == Some(std::ffi::OsStr::new("1"))
            || env::var_os(ENV_LEGACY_KEEP_WORKDIR).as_deref()
                == Some(std::ffi::OsStr::new("1"));
        Ok(Self { path, keep })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the directory on drop (post-listen failure path).
    pub(super) fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if self.keep {
            eprintln!("workdir: {}", self.path.display());
        } else {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
