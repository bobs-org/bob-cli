//! Shared Git plumbing used by vault maintenance commands.
//!
//! This module owns the exclusive maintenance lock plus the child environment
//! and `git -C <vault>` command builder used for unattended commits and pushes.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

/// Acquire the exclusive run lock shared by vault maintenance commands.
///
/// Returns `Ok(Some(file))` on success (hold the guard for the duration of the
/// run), `Err(0)` when another run already holds the lock, and `Err(1)` on an
/// unexpected I/O error.
pub(crate) fn acquire_lock() -> Result<Option<File>, i32> {
    acquire_lock_impl(false)
}

pub(crate) fn acquire_lock_quiet_if_held() -> Result<Option<File>, i32> {
    acquire_lock_impl(true)
}

fn acquire_lock_impl(quiet_if_held: bool) -> Result<Option<File>, i32> {
    let lock_file = lock_file_from_env().unwrap_or_else(default_lock_file);

    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_file)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "bob: could not open lock file {}: {error}",
                lock_file.display()
            );
            return Err(1);
        }
    };

    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            if !quiet_if_held {
                eprintln!(
                    "bob: another Bob vault maintenance run is already active; \
                     exiting."
                );
            }
            Err(0)
        }
        Err(error) => {
            eprintln!(
                "bob: could not acquire lock file {}: {error}",
                lock_file.display()
            );
            Err(1)
        }
    }
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
