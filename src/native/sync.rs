use std::{
    env,
    ffi::{OsStr, OsString},
    path::Path,
    process::Stdio,
};

use chrono::{Datelike, Local};

use super::{
    env as bob_env,
    ob::{self, ChildEnv},
};

const COMMAND_NAME: &str = "bob bulk-git-commit";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    if args.iter().any(|arg| {
        let value = arg.as_os_str();
        value == OsStr::new("--help") || value == OsStr::new("-h")
    }) {
        print_help();
        return 0;
    }

    if let Some(arg) = args.first() {
        eprintln!(
            "{COMMAND_NAME}: unexpected argument: {}",
            arg.to_string_lossy()
        );
        eprintln!("Try '{COMMAND_NAME} --help' for more information.");
        return 2;
    }

    let bob_dir = bob_env::bob_dir();

    let _lock = match ob::acquire_lock() {
        Ok(lock) => lock,
        Err(code) => return code,
    };

    let child_env = ob::child_env();
    commit_and_push_vault(&bob_dir, &child_env)
}

/// Stage, commit (if anything changed), and push the vault.
///
/// This does **not** acquire the run lock or run `ob sync`; the caller owns
/// both. `cronjob` calls this directly while holding the shared lock, and the
/// standalone `run()` wraps it after acquiring the lock itself.
pub(crate) fn commit_and_push_vault(
    bob_dir: &Path,
    child_env: &ChildEnv,
) -> i32 {
    let commit_message = commit_message();

    if let Err(message) = verify_bob_worktree(bob_dir, child_env) {
        return die(&message);
    }

    log(format_args!("Staging Bob vault changes."));
    if let Err(code) = run_git_command(bob_dir, child_env, ["add", "-A", "."]) {
        return code;
    }

    match git_diff_cached(bob_dir, child_env) {
        Ok(false) => {
            log(format_args!("No staged Bob changes to commit."));
        }
        Ok(true) => {
            log(format_args!("Committing Bob vault changes."));
            if let Err(code) =
                run_git_commit(bob_dir, child_env, &commit_message)
            {
                return code;
            }
        }
        Err(code) => return code,
    }

    log(format_args!("Pushing Bob vault commits."));
    if let Err(code) = run_git_command(bob_dir, child_env, ["push"]) {
        return code;
    }

    0
}

fn verify_bob_worktree(
    bob_dir: &Path,
    child_env: &ChildEnv,
) -> Result<(), String> {
    if !bob_dir.is_dir() {
        return Err(format!(
            "Bob directory does not exist: {}",
            bob_dir.display()
        ));
    }

    let status = ob::git_command(bob_dir, child_env)
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

fn run_git_command<const N: usize>(
    bob_dir: &Path,
    child_env: &ChildEnv,
    args: [&str; N],
) -> Result<(), i32> {
    let status = ob::git_command(bob_dir, child_env)
        .args(args)
        .status()
        .map_err(|error| die(&format!("failed to run git: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| bob_env::exit_code(status))
}

fn run_git_commit(
    bob_dir: &Path,
    child_env: &ChildEnv,
    message: &str,
) -> Result<(), i32> {
    let status = ob::git_command(bob_dir, child_env)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .status()
        .map_err(|error| die(&format!("failed to run git commit: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| bob_env::exit_code(status))
}

fn git_diff_cached(bob_dir: &Path, child_env: &ChildEnv) -> Result<bool, i32> {
    let status = ob::git_command(bob_dir, child_env)
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .arg("--exit-code")
        .status()
        .map_err(|error| die(&format!("failed to run git diff: {error}")))?;

    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(bob_env::exit_code(status)),
    }
}

fn today() -> String {
    let now = Local::now();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

fn commit_message() -> String {
    env::var("BOB_BULK_GIT_COMMIT_MESSAGE")
        .or_else(|_| env::var("BOB_SYNC_COMMIT_MESSAGE"))
        .unwrap_or_else(|_| format!("bob bulk-git-commit {}", today()))
}

fn log(args: std::fmt::Arguments<'_>) {
    println!("[{}] {args}", timestamp());
}

fn die(message: &str) -> i32 {
    eprintln!("[{}] ERROR: {message}", timestamp());
    1
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

fn print_help() {
    println!(
        "\
usage: {COMMAND_NAME}

Stage all Bob vault changes, commit if anything changed, and push.

environment:
  BOB_BULK_GIT_COMMIT_MESSAGE    override the Git commit message
  BOB_BULK_GIT_COMMIT_LOCK_FILE  override the shared lock path
  BOB_SYNC_COMMIT_MESSAGE        deprecated compatibility alias
  BOB_SYNC_LOCK_FILE             deprecated compatibility alias

options:
  -h, --help                     show this help message and exit"
    );
}
