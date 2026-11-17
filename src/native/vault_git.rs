//! Shared vault-git trackability helpers.
//!
//! Quiet `git check-ignore` probes shared by Keep imports and the
//! reference source store, so both report the same allowlist guidance
//! when a vault `.gitignore` would keep machine data out of git.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};

/// Quiet `git check-ignore` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoreQuiet {
    Ignored,
    Trackable,
}

/// A path that a verbose bulk ignore query reports as ignored, plus the
/// matching rule that would keep it local to this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IgnoreMatch {
    pub(crate) path: PathBuf,
    pub(crate) rule: String,
}

/// Query several vault-relative paths in one `git check-ignore` process.
/// Non-matches and re-inclusion rules are omitted from the result.
pub(crate) fn check_ignored_many(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    relatives: &[PathBuf],
) -> Result<Vec<IgnoreMatch>, String> {
    if relatives.is_empty() {
        return Ok(Vec::new());
    }
    let mut child =
        crate::native::ob::git_check_ignore_command(bob_dir, child_env)
            .args(["--verbose", "--non-matching", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!("failed to run git check-ignore: {error}")
            })?;
    let input_result = if let Some(stdin) = child.stdin.as_mut() {
        relatives.iter().try_for_each(|relative| {
            writeln!(stdin, "{}", relative.to_string_lossy())
        })
    } else {
        Ok(())
    };
    if let Err(error) = input_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("write git check-ignore input: {error}"));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for git check-ignore: {error}"))?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!(
                "git check-ignore exited {}",
                output.status.code().unwrap_or(-1)
            )
        } else {
            detail
        });
    }

    let mut matches = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((rule, path)) = line.split_once('\t') else {
            continue;
        };
        let pattern = rule.rsplit(':').next().unwrap_or_default();
        if pattern.is_empty() || pattern.starts_with('!') {
            continue;
        }
        matches.push(IgnoreMatch {
            path: PathBuf::from(path),
            rule: rule.to_string(),
        });
    }
    Ok(matches)
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
