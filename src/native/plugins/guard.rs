//! Guard against bare `bob plugins sync` from a foreign bob-plugins checkout.
//!
//! `bob plugins sync` without `--repo` deploys the resolved repo
//! (`BOB_PLUGINS_DIR`, else `~/projects/github/bobs-org/bob-plugins`). When
//! the caller runs it from inside a *different* bob-plugins checkout — for
//! example a SASE linked worktree — the sync silently deploys the canonical
//! checkout instead, rolling back whatever the worktree just deployed
//! (bob-cli-59). The guard aborts before any pull or copy in that case.

use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

/// A bob-plugins checkout the cwd sits inside that is not the resolved repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ForeignCheckout {
    /// Canonical toplevel of the checkout the cwd sits inside.
    pub(super) toplevel: PathBuf,
}

/// Refuse a bare sync when the cwd is inside a bob-plugins checkout whose
/// toplevel differs from the resolved repo.
///
/// Returns `None` — sync proceeds — when `--repo` was explicit (callers pass
/// the checkout they mean), when the cwd is outside any Git checkout, when
/// the enclosing checkout is not a bob-plugins checkout, or when the cwd is
/// inside the resolved checkout itself. Runs before any pull or copy, and in
/// `--dry-run` too.
pub(super) fn check_bare_sync(
    resolved_repo: &Path,
    repo_is_explicit: bool,
) -> Option<ForeignCheckout> {
    if repo_is_explicit {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let toplevel = git_toplevel(&cwd)?;
    if same_path(&toplevel, resolved_repo) {
        return None;
    }
    if !is_bob_plugins_checkout(&toplevel) {
        return None;
    }
    Some(ForeignCheckout { toplevel })
}

/// Error text naming both checkouts and the command to run instead.
pub(super) fn refusal_message(
    resolved_repo: &Path,
    foreign: &ForeignCheckout,
) -> String {
    format!(
        "refusing bare sync from inside a different bob-plugins checkout ({}); \
the sync would deploy {} instead. Run `bob plugins sync --repo {} [-p <id>]` \
to deploy this checkout, or run the bare sync from outside any bob-plugins checkout.",
        foreign.toplevel.display(),
        resolved_repo.display(),
        foreign.toplevel.display(),
    )
}

/// Toplevel of the Git checkout containing `cwd`, or `None` outside one.
fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if toplevel.is_empty() {
        return None;
    }
    Some(canonicalize(&PathBuf::from(toplevel)))
}

/// Whether `toplevel` is a bob-plugins checkout: its `origin` remote points
/// at `bobs-org/bob-plugins`, or it carries the stable repo-root marker
/// (`plugins/` plus the monorepo `package.json`).
fn is_bob_plugins_checkout(toplevel: &Path) -> bool {
    if origin_points_at_bob_plugins(toplevel) {
        return true;
    }
    toplevel.join("plugins").is_dir() && toplevel.join("package.json").is_file()
}

fn origin_points_at_bob_plugins(toplevel: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .contains("bobs-org/bob-plugins")
        }
        _ => false,
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    canonicalize(left) == canonicalize(right)
}

fn canonicalize(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            path.to_path_buf()
        }
        Err(_) => path.to_path_buf(),
    }
}
