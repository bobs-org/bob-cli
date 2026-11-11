//! Resolve the bob-plugins repo from the enclosing checkout.
//!
//! `bob plugins sync` without `--repo` deploys the resolved repo. When the
//! caller runs it from inside a bob-plugins checkout — for example a SASE
//! linked worktree — the enclosing checkout wins over `BOB_PLUGINS_DIR` and
//! the default, matching git's own semantics and the documented workflow
//! "make changes to this linked repo and, when done, run `bob plugins sync`".

use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::native::env as bob_env;

use super::model::RepoSource;

/// Resolved repo root plus where it came from.
///
/// Resolution order: `-r/--repo` (handled by the caller) is not part of this
/// helper; this helper resolves steps 2–4: the enclosing bob-plugins checkout
/// when the cwd sits inside one, else `BOB_PLUGINS_DIR`, else the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedRepo {
    pub(super) repo: PathBuf,
    pub(super) source: RepoSource,
    /// Set when the enclosing checkout wins over a different env/default
    /// repo; the caller prints it once on stderr.
    pub(super) notice: Option<String>,
}

/// Resolve steps 2–4 of the repo resolution order.
pub(super) fn resolve_default_repo() -> ResolvedRepo {
    let (fallback_repo, fallback_source) = fallback_repo();
    let Some(toplevel) = enclosing_bob_plugins_checkout() else {
        return ResolvedRepo {
            repo: fallback_repo,
            source: fallback_source,
            notice: None,
        };
    };
    if same_path(&toplevel, &fallback_repo) {
        // The cwd sits inside the env/default checkout: resolution is
        // unchanged, so no notice.
        return ResolvedRepo {
            repo: fallback_repo,
            source: fallback_source,
            notice: None,
        };
    }
    let notice = format!(
        "bob plugins: using enclosing checkout {} (default is {}; pass --repo to override)",
        toplevel.display(),
        fallback_repo.display(),
    );
    ResolvedRepo {
        repo: toplevel,
        source: RepoSource::EnclosingCheckout,
        notice: Some(notice),
    }
}

/// Steps 3–4: `BOB_PLUGINS_DIR`, else the default checkout.
fn fallback_repo() -> (PathBuf, RepoSource) {
    let explicit =
        bob_env::var_os("BOB_PLUGINS_DIR").filter(|value| !value.is_empty());
    match explicit {
        Some(value) => (
            bob_env::expand_tilde(&PathBuf::from(value)),
            RepoSource::Env,
        ),
        None => (bob_env::plugins_dir(), RepoSource::Default),
    }
}

/// Toplevel of the enclosing bob-plugins checkout, or `None` when the cwd is
/// outside any Git checkout or the checkout is not a bob-plugins checkout.
fn enclosing_bob_plugins_checkout() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let toplevel = git_toplevel(&cwd)?;
    if !is_bob_plugins_checkout(&toplevel) {
        return None;
    }
    Some(toplevel)
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
