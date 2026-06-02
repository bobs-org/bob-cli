//! Shared Obsidian (`ob`) and Git plumbing used by the nightly `cronjob`
//! orchestrator and the wrapped `sync` / `collect-done` commands.
//!
//! This module owns the single source of truth for three things that used to
//! be duplicated (and divergent) across `sync.rs` and `collect_done.rs`:
//!
//! 1. Discovering the `ob` binary (honoring `OB_COMMAND`, then `PATH`, then an
//!    NVM fallback).
//! 2. Running `ob sync` / `ob sync-status` against the vault.
//! 3. The child environment (ssh-agent vars + non-interactive `GIT_SSH_COMMAND`)
//!    and the `git -C <vault>` command builder used for commits and pushes.
//!
//! It also owns the exclusive run lock so the standalone `sync` command and the
//! `cronjob` orchestrator share a single mutual-exclusion gate.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use fs2::FileExt;

use super::env as bob_env;

const SYNC_ALREADY_RUNNING_MESSAGE: &str =
    "Another sync instance is already running for this vault.";

/// Environment variables injected into every `ob` / `git` child process.
pub(crate) type ChildEnv = Vec<(OsString, OsString)>;

/// Outcome of the shared `ob sync` gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    /// `ob sync` ran successfully.
    Ran,
    /// The `ob` binary could not be located; sync was skipped (non-fatal).
    SkippedMissingCommand,
    /// Another sync is already running for this vault (non-fatal).
    AlreadyRunning,
}

/// Run `ob sync --path <vault>` followed by `ob sync-status --path <vault>`.
///
/// "Already running" and a missing `ob` binary are treated as non-fatal,
/// matching the historical behavior of the wrapped commands. A genuine sync
/// failure returns the process exit code via `Err`.
pub(crate) fn sync_vault(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<SyncOutcome, i32> {
    let Some(ob_command) = load_ob_command() else {
        return Ok(SyncOutcome::SkippedMissingCommand);
    };

    let output = ob_command_builder(&ob_command, child_env)
        .arg("sync")
        .arg("--path")
        .arg(vault)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SyncOutcome::SkippedMissingCommand);
        }
        Err(error) => {
            eprintln!("bob: failed to run ob sync: {error}");
            return Err(1);
        }
    };

    let sync_output = merged_output(&output);
    if !output.status.success() {
        if sync_output.contains(SYNC_ALREADY_RUNNING_MESSAGE) {
            return Ok(SyncOutcome::AlreadyRunning);
        }
        write_stderr_output(&sync_output);
        return Err(bob_env::exit_code(output.status));
    }

    write_stdout_output(&sync_output);
    run_sync_status(&ob_command, child_env, vault);
    Ok(SyncOutcome::Ran)
}

/// Run `ob sync-status` for visibility. Its result is informational only, so a
/// failure here never aborts the run.
fn run_sync_status(ob_command: &OsStr, child_env: &ChildEnv, vault: &Path) {
    let output = ob_command_builder(ob_command, child_env)
        .arg("sync-status")
        .arg("--path")
        .arg(vault)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    if let Ok(output) = output {
        let status_output = merged_output(&output);
        if output.status.success() {
            write_stdout_output(&status_output);
        } else {
            write_stderr_output(&status_output);
        }
    }
}

fn ob_command_builder(ob_command: &OsStr, child_env: &ChildEnv) -> Command {
    let mut command = Command::new(ob_command);
    command.envs(child_env.iter().cloned());
    command
}

/// Locate the `ob` binary: `OB_COMMAND` first (used by the test suite), then
/// `PATH`, then an NVM fallback.
fn load_ob_command() -> Option<OsString> {
    if let Some(value) =
        env::var_os("OB_COMMAND").filter(|value| !value.is_empty())
    {
        return Some(value);
    }

    if let Some(path) = find_command_in_path("ob") {
        return Some(path.into_os_string());
    }

    load_ob_from_nvm().map(PathBuf::into_os_string)
}

fn load_ob_from_nvm() -> Option<PathBuf> {
    let script = r#"
set +u
export NVM_DIR="${NVM_DIR:-${HOME}/.config/nvm}"
if [ -r "${NVM_DIR}/nvm.sh" ]; then
  . "${NVM_DIR}/nvm.sh"
fi
if ! command -v ob >/dev/null 2>&1 && command -v nvm >/dev/null 2>&1; then
  nvm use --silent default >/dev/null || true
fi
command -v ob
"#;

    let output = Command::new("bash")
        .arg("-lc")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn find_command_in_path(command: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        candidate.is_file().then_some(candidate)
    })
}

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

/// Acquire the exclusive run lock shared by `sync` and `cronjob`.
///
/// Returns `Ok(Some(file))` on success (hold the guard for the duration of the
/// run), `Err(0)` when another run already holds the lock, and `Err(1)` on an
/// unexpected I/O error.
pub(crate) fn acquire_lock() -> Result<Option<File>, i32> {
    let lock_file = env::var_os("BOB_SYNC_LOCK_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_lock_file);

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
            eprintln!("bob: another sync run is already active; exiting.");
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

fn default_lock_file() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bob_sync.lock")
}

fn merged_output(output: &Output) -> String {
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_stdout_output(output: &str) {
    if output.is_empty() {
        return;
    }

    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
}

fn write_stderr_output(output: &str) {
    if output.is_empty() {
        return;
    }

    eprint!("{output}");
    if !output.ends_with('\n') {
        eprintln!();
    }
}
