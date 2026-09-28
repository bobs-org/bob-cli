//! Shared Git plumbing used by vault maintenance commands.
//!
//! This module owns the exclusive maintenance lock plus the child environment
//! and `git -C <vault>` command builder used for unattended commits and pushes.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use fs2::FileExt;

use super::env as bob_env;

/// Environment variables injected into every `git` child process.
pub(crate) type ChildEnv = Vec<(OsString, OsString)>;

/// Build a `git -C <vault>` command carrying the shared child environment so
/// pushes are non-interactive under cron.
pub(crate) fn git_command(vault: &Path, child_env: &ChildEnv) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(vault).envs(child_env.iter().cloned());
    command
}

/// Collect the environment injected into every child process: the ssh-agent
/// variables plus a non-interactive `GIT_SSH_COMMAND` (unless one is already
/// set).
pub(crate) fn child_env() -> ChildEnv {
    let mut values = source_ssh_agent_env();

    if env::var_os("GIT_SSH_COMMAND").is_none()
        && !values
            .iter()
            .any(|(key, _)| key == OsStr::new("GIT_SSH_COMMAND"))
    {
        values.push((
            OsString::from("GIT_SSH_COMMAND"),
            OsString::from("ssh -o BatchMode=yes"),
        ));
    }

    values
}

fn source_ssh_agent_env() -> ChildEnv {
    let source_file = bob_env::home_dir().join(".ssh-agent-thing");
    if fs::metadata(&source_file).is_err() {
        return Vec::new();
    }

    let script = r#"
set +u
. "$1" >/dev/null
env -0
"#;

    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .arg("bob-sync")
        .arg(&source_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|byte| *byte == b'=')?;
            let key = OsString::from(
                String::from_utf8_lossy(&entry[..equals]).into_owned(),
            );
            let value = OsString::from(
                String::from_utf8_lossy(&entry[equals + 1..]).into_owned(),
            );
            Some((key, value))
        })
        .collect()
}

pub(crate) fn verify_bob_worktree(
    bob_dir: &Path,
    child_env: &ChildEnv,
) -> Result<(), String> {
    if !bob_dir.is_dir() {
        return Err(format!(
            "Bob directory does not exist: {}",
            bob_dir.display()
        ));
    }

    let status = git_command(bob_dir, child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run git rev-parse: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Bob directory is not a Git worktree: {}",
            bob_dir.display()
        ))
    }
}

/// Outcome of acquiring the shared vault-maintenance lock without printing.
#[derive(Debug)]
pub(crate) enum LockAcquireError {
    Contended,
    Open { path: PathBuf, error: io::Error },
    Acquire { path: PathBuf, error: io::Error },
}

/// Acquire the exclusive run lock shared by vault maintenance commands.
///
/// Returns `Ok(Some(file))` on success (hold the guard for the duration of the
/// run), `Err(0)` when another run already holds the lock, and `Err(1)` on an
/// unexpected I/O error.
pub(crate) fn acquire_lock() -> Result<Option<File>, i32> {
    report_lock(try_acquire_lock(), false)
}

pub(crate) fn acquire_lock_quiet_if_held() -> Result<Option<File>, i32> {
    report_lock(try_acquire_lock(), true)
}

/// Acquire the shared lock without printing so callers can format contention
/// in their own output. `BOB_VAULT_SYNC_LOCK_FILE` is still honored.
pub(crate) fn try_acquire_lock() -> Result<File, LockAcquireError> {
    let lock_file = lock_file_from_env().unwrap_or_else(default_lock_file);

    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_file)
        .map_err(|error| LockAcquireError::Open {
            path: lock_file.clone(),
            error,
        })?;

    match file.try_lock_exclusive() {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err(LockAcquireError::Contended)
        }
        Err(error) => Err(LockAcquireError::Acquire {
            path: lock_file,
            error,
        }),
    }
}

fn report_lock(
    result: Result<File, LockAcquireError>,
    quiet_if_held: bool,
) -> Result<Option<File>, i32> {
    match result {
        Ok(file) => Ok(Some(file)),
        Err(LockAcquireError::Contended) => {
            if !quiet_if_held {
                eprintln!(
                    "bob: another Bob vault maintenance run is already active; \
                     exiting."
                );
            }
            Err(0)
        }
        Err(LockAcquireError::Open { path, error }) => {
            eprintln!(
                "bob: could not open lock file {}: {error}",
                path.display()
            );
            Err(1)
        }
        Err(LockAcquireError::Acquire { path, error }) => {
            eprintln!(
                "bob: could not acquire lock file {}: {error}",
                path.display()
            );
            Err(1)
        }
    }
}

/// Failure modes for the bounded maintenance-lock wait.
// Temporary until the `randomize` command phase wires in the plumbing.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum LockWaitError {
    Timeout { path: PathBuf },
    Open { path: PathBuf, error: io::Error },
    Acquire { path: PathBuf, error: io::Error },
}

impl std::fmt::Display for LockWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { path } => write!(
                f,
                "another Bob vault maintenance run is already active (waited on {})",
                path.display()
            ),
            Self::Open { path, error } => write!(
                f,
                "could not open lock file {}: {error}",
                path.display()
            ),
            Self::Acquire { path, error } => write!(
                f,
                "could not acquire lock file {}: {error}",
                path.display()
            ),
        }
    }
}

/// Acquire the shared vault-maintenance lock, waiting up to `timeout` while
/// another run holds it.
///
/// Polls [`try_acquire_lock`] with short sleeps (about 250 ms).
/// `on_first_wait` runs exactly once, when the first poll finds the lock
/// contended, so callers can print a single "waiting…" line. A zero `timeout`
/// fails fast without waiting.
#[allow(dead_code)]
pub(crate) fn acquire_lock_waiting(
    timeout: Duration,
    on_first_wait: impl FnOnce(),
) -> Result<File, LockWaitError> {
    let started = Instant::now();
    let mut on_first_wait = Some(on_first_wait);
    loop {
        match try_acquire_lock() {
            Ok(file) => return Ok(file),
            Err(LockAcquireError::Contended) => {
                if let Some(callback) = on_first_wait.take() {
                    callback();
                }
                if started.elapsed() >= timeout {
                    return Err(LockWaitError::Timeout {
                        path: lock_file_from_env()
                            .unwrap_or_else(default_lock_file),
                    });
                }
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(remaining.min(Duration::from_millis(250)));
            }
            Err(LockAcquireError::Open { path, error }) => {
                return Err(LockWaitError::Open { path, error });
            }
            Err(LockAcquireError::Acquire { path, error }) => {
                return Err(LockWaitError::Acquire { path, error });
            }
        }
    }
}

/// Tell "not a Git worktree" apart from "git is missing".
///
/// Returns `Ok(true)` when `vault` is inside a worktree, `Ok(false)` when git
/// runs but reports it is not, and `Err` only when the git command itself
/// cannot be started.
#[allow(dead_code)]
pub(crate) fn detect_git_worktree(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<bool, String> {
    let status = git_command(vault, child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run git rev-parse: {error}"))?;
    Ok(status.success())
}

/// Commit exactly `paths` with `message`, leaving other dirty or already-staged
/// files untouched.
///
/// Stages with `git add -- <paths>`, then checks the scoped cached diff:
/// returns `Ok(None)` when the listed paths hold no staged change. Otherwise
/// commits with `git commit -F - -- <paths>` (message on stdin) and returns
/// the new `HEAD` sha.
#[allow(dead_code)]
pub(crate) fn commit_paths(
    vault: &Path,
    child_env: &ChildEnv,
    message: &str,
    paths: &[PathBuf],
) -> Result<Option<String>, String> {
    if paths.is_empty() {
        return Ok(None);
    }

    let add_status = git_command(vault, child_env)
        .arg("add")
        .arg("--")
        .args(paths)
        .status()
        .map_err(|error| format!("failed to run git add: {error}"))?;
    if !add_status.success() {
        return Err(format!("git add -- <{} path(s)> failed", paths.len()));
    }

    let diff_status = git_command(vault, child_env)
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .arg("--")
        .args(paths)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run git diff --cached: {error}"))?;
    if diff_status.success() {
        return Ok(None);
    }

    let mut commit = git_command(vault, child_env);
    commit
        .arg("commit")
        .arg("-F")
        .arg("-")
        .arg("--")
        .args(paths)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = commit
        .spawn()
        .map_err(|error| format!("failed to run git commit: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "git commit has no stdin".to_string())?
        .write_all(message.as_bytes())
        .map_err(|error| format!("failed to write commit message: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for git commit: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let sha_output = git_command(vault, child_env)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .map_err(|error| {
            format!("failed to run git rev-parse HEAD: {error}")
        })?;
    if !sha_output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&sha_output.stderr).trim()
        ));
    }
    Ok(Some(
        String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string(),
    ))
}

fn lock_file_from_env() -> Option<PathBuf> {
    env::var_os("BOB_VAULT_SYNC_LOCK_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_lock_file() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bob_sync.lock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    /// Run `f` with the lock file pointed at a unique temp path.
    ///
    /// This is the only test in the suite that writes
    /// `BOB_VAULT_SYNC_LOCK_FILE`, so no concurrent test can disagree on it.
    fn with_test_lock_file(tag: &str, f: impl FnOnce(&Path)) {
        let path = env::temp_dir().join(format!(
            "bob-ob-lock-test-{}-{tag}.lock",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let old = env::var_os("BOB_VAULT_SYNC_LOCK_FILE");
        unsafe {
            env::set_var("BOB_VAULT_SYNC_LOCK_FILE", &path);
        }
        f(&path);
        unsafe {
            match old {
                Some(old) => env::set_var("BOB_VAULT_SYNC_LOCK_FILE", old),
                None => env::remove_var("BOB_VAULT_SYNC_LOCK_FILE"),
            }
        }
        let _ = fs::remove_file(&path);
    }

    // One test (not three) touches `BOB_VAULT_SYNC_LOCK_FILE`, because the
    // variable is process-global and parallel tests would race on it.
    #[test]
    fn lock_wait_behavior() {
        with_test_lock_file("wait", |_| {
            let waits = Arc::new(AtomicUsize::new(0));

            // While held, a short budget times out and the callback fires once.
            let held = try_acquire_lock().expect("initial lock");
            let waits_before = Arc::clone(&waits);
            let result = acquire_lock_waiting(Duration::from_millis(400), {
                let waits_before = Arc::clone(&waits_before);
                move || {
                    waits_before.fetch_add(1, Ordering::SeqCst);
                }
            });
            assert!(
                matches!(result, Err(LockWaitError::Timeout { .. })),
                "expected timeout, got {result:?}"
            );
            assert_eq!(waits_before.load(Ordering::SeqCst), 1);
            drop(held);

            // A waiter blocked while held acquires after release, firing the
            // callback exactly once more.
            let held = try_acquire_lock().expect("re-acquire lock");
            let handle = std::thread::spawn({
                let waits = Arc::clone(&waits);
                move || {
                    acquire_lock_waiting(Duration::from_secs(10), move || {
                        waits.fetch_add(1, Ordering::SeqCst);
                    })
                }
            });
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
            let acquired = handle.join().expect("waiter thread").expect("lock");
            drop(acquired);
            assert_eq!(
                waits.load(Ordering::SeqCst),
                2,
                "callback must fire exactly once per wait"
            );

            // Uncontended, the lock is acquired without waiting or calling back.
            let _guard = acquire_lock_waiting(Duration::ZERO, || {
                waits.fetch_add(1, Ordering::SeqCst);
            })
            .expect("lock");
            assert_eq!(waits.load(Ordering::SeqCst), 2);
        });
    }

    fn git_env() -> ChildEnv {
        Vec::new()
    }

    fn init_repo(vault: &Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(vault)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        fs::write(vault.join("a.md"), "a\n").unwrap();
        fs::write(vault.join("b.md"), "b\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "initial"]);
    }

    fn git_names(vault: &Path, args: &[&str]) -> Vec<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn commit_paths_scopes_to_listed_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir(&vault).unwrap();
        init_repo(&vault);
        let env = git_env();

        assert!(detect_git_worktree(&vault, &env).expect("worktree check"));

        // a.md dirty, b.md staged, c.md dirty and untracked.
        fs::write(vault.join("a.md"), "a-changed\n").unwrap();
        fs::write(vault.join("b.md"), "b-changed\n").unwrap();
        fs::write(vault.join("c.md"), "c\n").unwrap();
        assert!(git_names(&vault, &["add", "b.md"]).is_empty());

        let sha = commit_paths(
            &vault,
            &env,
            "scoped commit",
            &[PathBuf::from("a.md")],
        )
        .expect("commit")
        .expect("sha");
        assert!(!sha.is_empty());

        assert_eq!(
            git_names(&vault, &["show", "--name-only", "--format="]),
            ["a.md"]
        );
        // The staged b.md change and the dirty c.md file stay uncommitted.
        assert_eq!(
            git_names(&vault, &["diff", "--cached", "--name-only"]),
            ["b.md"]
        );
        assert_eq!(fs::read_to_string(vault.join("c.md")).unwrap(), "c\n");
        assert_eq!(
            fs::read_to_string(vault.join("b.md")).unwrap(),
            "b-changed\n"
        );

        // Nothing left to commit for a.md.
        assert_eq!(
            commit_paths(&vault, &env, "noop", &[PathBuf::from("a.md")])
                .expect("noop"),
            None
        );
        assert_eq!(
            commit_paths(&vault, &env, "empty", &[]).expect("empty"),
            None
        );
    }

    #[test]
    fn detect_git_worktree_reports_non_worktree() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(!detect_git_worktree(temp.path(), &git_env()).expect("check"));
    }
}
