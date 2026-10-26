//! Shared vault-git trackability helpers.
//!
//! Quiet `git check-ignore` probes shared by Keep imports and the
//! reference source store, so both report the same allowlist guidance
//! when a vault `.gitignore` would keep machine data out of git.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

/// Quiet `git check-ignore` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoreQuiet {
    Ignored,
    Trackable,
}

/// Run `git check-ignore --quiet` for one vault-relative path.
pub(crate) fn check_ignore_quiet(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
) -> Result<IgnoreQuiet, String> {
    let output = crate::native::ob::git_command(bob_dir, child_env)
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(rel)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git check-ignore: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(IgnoreQuiet::Ignored),
        Some(1) => Ok(IgnoreQuiet::Trackable),
        Some(code) => {
            let detail =
                String::from_utf8_lossy(&output.stderr).trim().to_string();
            if detail.is_empty() {
                Err(format!(
                    "cannot inspect Git ignore rules for {}: git check-ignore exited {code}",
                    rel.display()
                ))
            } else {
                Err(format!(
                    "cannot inspect Git ignore rules for {}: {detail}",
                    rel.display()
                ))
            }
        }
        None => Err(format!(
            "cannot inspect Git ignore rules for {}: git check-ignore terminated by signal",
            rel.display()
        )),
    }
}

/// Boolean probe: `true` when the path is ignored.
pub(crate) fn is_ignored(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
) -> bool {
    matches!(
        check_ignore_quiet(bob_dir, child_env, rel),
        Ok(IgnoreQuiet::Ignored)
    )
}

/// Hint for inspecting the matching rule.
#[allow(dead_code)]
pub(crate) fn inspect_hint(bob_dir: &Path, rel: &Path) -> String {
    format!(
        "git -C {} check-ignore -v -- {}",
        bob_dir.display(),
        rel.display()
    )
}

#[allow(dead_code)]
pub(crate) fn trackability_probe(rel: &Path) -> PathBuf {
    rel.to_path_buf()
}
