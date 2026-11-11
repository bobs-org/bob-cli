//! Shared vault-git trackability helpers.
//!
//! Quiet `git check-ignore` probes shared by Keep imports and the
//! reference source store, so both report the same allowlist guidance
//! when a vault `.gitignore` would keep machine data out of git.

use std::{path::Path, process::Stdio};

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
    if rel.as_os_str().as_encoded_bytes().first() == Some(&b':') {
        return Err(format!(
            "cannot inspect Git ignore rules for {}: path starts with ':' and would be read as pathspec magic",
            rel.display()
        ));
    }
    let output =
        crate::native::ob::git_check_ignore_command(bob_dir, child_env)
            .arg("--quiet")
            .arg("--")
            .arg(rel)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| {
                format!("failed to run git check-ignore: {error}")
            })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn check_ignore_quiet_handles_ignored_and_trackable_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path();
        let status = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["init", "--quiet"])
            .status()
            .expect("git init");
        assert!(status.success());
        fs::write(vault.join(".gitignore"), "ignored.md\n").expect("gitignore");
        let child_env = Vec::new();

        assert_eq!(
            check_ignore_quiet(vault, &child_env, Path::new("ignored.md")),
            Ok(IgnoreQuiet::Ignored)
        );
        assert_eq!(
            check_ignore_quiet(vault, &child_env, Path::new("trackable.md")),
            Ok(IgnoreQuiet::Trackable)
        );
    }

    #[test]
    fn check_ignore_quiet_rejects_leading_colon_paths() {
        let child_env = Vec::new();
        let error = check_ignore_quiet(
            Path::new("/unused"),
            &child_env,
            Path::new(":secret.md"),
        )
        .expect_err("leading colon must not be interpreted as pathspec magic");
        assert!(error.contains("cannot inspect Git ignore rules"), "{error}");
    }
}
