use std::{
    io,
    path::Path,
    process::{Command, Output},
};

use crate::native::{env as bob_env, ob};

use super::model::COMMAND_NAME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PullOutcome {
    Skipped,
    Pulled { summary: String },
    Failed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitDetection {
    Worktree,
    NotWorktree,
    MissingGit,
}

pub(super) fn pull_repo(repo: &Path) -> PullOutcome {
    let child_env = ob::child_env();
    match detect_git_worktree(repo, &child_env) {
        GitDetection::Worktree => {}
        GitDetection::NotWorktree | GitDetection::MissingGit => {
            return PullOutcome::Skipped;
        }
    }

    let output = ob::git_command(repo, &child_env)
        .arg("pull")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();

    match output {
        Ok(output) if output.status.success() => PullOutcome::Pulled {
            summary: summarize_git_pull(&output),
        },
        Ok(output) => PullOutcome::Failed {
            message: git_pull_failure_message(&output),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            PullOutcome::Skipped
        }
        Err(error) => PullOutcome::Failed {
            message: format!("failed to run git pull: {error}"),
        },
    }
}

fn detect_git_worktree(repo: &Path, child_env: &ob::ChildEnv) -> GitDetection {
    let output = ob::git_command(repo, child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output();

    match output {
        Ok(output)
            if output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true" =>
        {
            GitDetection::Worktree
        }
        Ok(output) if output.status.success() => GitDetection::NotWorktree,
        Ok(_) => GitDetection::NotWorktree,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            GitDetection::MissingGit
        }
        Err(_) => GitDetection::NotWorktree,
    }
}

pub(super) fn print_pull_outcome(outcome: PullOutcome) {
    match outcome {
        PullOutcome::Skipped => {}
        PullOutcome::Pulled { summary } => {
            if !is_up_to_date_summary(&summary) {
                eprintln!("{COMMAND_NAME}: git pull: {summary}");
            }
        }
        PullOutcome::Failed { message } => {
            eprintln!(
                "{COMMAND_NAME}: warning: git pull failed; using existing checkout: {message}"
            );
        }
    }
}

fn summarize_git_pull(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return "completed".to_string();
    }

    if let Some(line) = lines.iter().find(|line| is_up_to_date_summary(line)) {
        return (*line).to_string();
    }

    if lines.contains(&"Fast-forward") {
        if let Some(stat) = lines
            .iter()
            .rev()
            .find(|line| line.contains("file changed"))
        {
            return format!("Fast-forward ({stat})");
        }
        return "Fast-forward".to_string();
    }

    lines[0].to_string()
}

fn is_up_to_date_summary(summary: &str) -> bool {
    matches!(summary, "Already up to date." | "Already up-to-date.")
}

fn git_pull_failure_message(output: &Output) -> String {
    let stderr = one_line_output(&output.stderr);
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = one_line_output(&output.stdout);
    if !stdout.is_empty() {
        return stdout;
    }

    format!(
        "git pull exited with code {}",
        bob_env::exit_code(output.status)
    )
}

fn one_line_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Reports whether `vault_file` has uncommitted changes in the vault Git repo.
///
/// Uses `git status --porcelain` scoped to the single file. A vault that is not
/// a Git repo, or an unavailable `git`, yields `false`: there is no committed
/// state to protect, so the copy proceeds.
pub(super) fn vault_file_is_dirty(bob_dir: &Path, vault_file: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .arg(vault_file)
        .output();
    match output {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => false,
    }
}
