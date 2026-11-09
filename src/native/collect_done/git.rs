//! Git worktree detection and the commit/push lifecycle as structured
//! outcomes.
//!
//! Nothing here prints to stdout: every step records its result in a
//! [`GitReport`] so human and JSON rendering read the same facts. Subprocess
//! diagnostics go to stderr only.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitOutcome {
    /// Git was not touched (dry run, empty plan, or an earlier failure).
    NotRun,
    /// Git ran far enough to decide there was nothing to commit or no
    /// worktree to commit into.
    Skipped,
    /// The touched paths were committed and pushed.
    Pushed,
    /// A Git step failed; the vault changes were already applied.
    Failed,
}

impl GitOutcome {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotRun => "not_run",
            Self::Skipped => "skipped",
            Self::Pushed => "pushed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitFailure {
    pub(super) step: &'static str,
    pub(super) message: String,
    pub(super) exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitReport {
    pub(super) outcome: GitOutcome,
    pub(super) skip_reason: Option<&'static str>,
    pub(super) warning: Option<String>,
    pub(super) detected_worktree: bool,
    pub(super) staged_paths: Option<usize>,
    pub(super) committed: bool,
    pub(super) commit_message: Option<String>,
    pub(super) pushed: bool,
    pub(super) paths: Vec<PathBuf>,
    pub(super) failure: Option<GitFailure>,
}

impl GitReport {
    pub(super) fn not_run(reason: &'static str) -> Self {
        Self {
            outcome: GitOutcome::NotRun,
            skip_reason: Some(reason),
            warning: None,
            detected_worktree: false,
            staged_paths: None,
            committed: false,
            commit_message: None,
            pushed: false,
            paths: Vec::new(),
            failure: None,
        }
    }

    fn skipped(reason: &'static str) -> Self {
        Self {
            outcome: GitOutcome::Skipped,
            ..Self::not_run(reason)
        }
    }

    /// The Git step was deliberately not needed: nothing was changed.
    pub(super) fn no_changes() -> Self {
        Self::skipped("no_changes")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitDetection {
    Worktree,
    NotWorktree,
    MissingGit,
}

/// Commit exactly `touched_paths` and push, reporting every step.
///
/// Only called after a complete guarded apply. A non-worktree vault or a
/// missing `git` skips with a warning; a failing step stops the lifecycle and
/// is recorded as a failure — the vault changes are already on disk and the
/// archive mutation is never replayed.
pub(super) fn finish_git(
    vault: &Path,
    child_env: &ChildEnv,
    touched_paths: Vec<PathBuf>,
) -> GitReport {
    let mut report = GitReport::skipped("not_worktree");
    report.paths = touched_paths.clone();
    match detect_git_worktree(vault, child_env) {
        Ok(GitDetection::Worktree) => {
            report.detected_worktree = true;
            report.outcome = GitOutcome::NotRun;
            report.skip_reason = None;
        }
        Ok(GitDetection::NotWorktree) => {
            report.warning = Some(
                "vault is not a git worktree; skipping commit and push"
                    .to_string(),
            );
            return report;
        }
        Ok(GitDetection::MissingGit) => {
            report.skip_reason = Some("git_missing");
            report.warning = Some(
                "git command not found; skipping commit and push".to_string(),
            );
            return report;
        }
        Err(failure) => return failed(report, failure),
    }

    if let Err(failure) = stage_git_paths(vault, child_env, &touched_paths) {
        return failed(report, failure);
    }
    report.staged_paths = Some(touched_paths.len());

    match git_has_staged_changes(vault, child_env, &touched_paths) {
        Ok(true) => {}
        Ok(false) => {
            report.outcome = GitOutcome::Skipped;
            report.skip_reason = Some("nothing_to_commit");
            return report;
        }
        Err(failure) => return failed(report, failure),
    }

    let message = collect_done_commit_message();
    if let Err(failure) =
        commit_git_paths(vault, child_env, &message, &touched_paths)
    {
        return failed(report, failure);
    }
    report.committed = true;
    report.commit_message = Some(message);
    if let Err(failure) = push_git(vault, child_env) {
        return failed(report, failure);
    }
    report.pushed = true;
    report.outcome = GitOutcome::Pushed;
    report
}

fn failed(mut report: GitReport, failure: GitFailure) -> GitReport {
    report.outcome = GitOutcome::Failed;
    report.skip_reason = None;
    report.failure = Some(failure);
    report
}

pub(super) fn detect_git_worktree(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<GitDetection, GitFailure> {
    let output = ob::git_command(vault, child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(GitDetection::Worktree),
        Ok(_) => Ok(GitDetection::NotWorktree),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(GitDetection::MissingGit)
        }
        Err(error) => Err(spawn_failure("git rev-parse", &error)),
    }
}

/// Vault-relative touched paths, sorted and deduplicated.
pub(super) fn touched_git_paths(applied: &[PathBuf]) -> Vec<PathBuf> {
    applied
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn stage_git_paths(
    vault: &Path,
    child_env: &ChildEnv,
    paths: &[PathBuf],
) -> Result<(), GitFailure> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("add").arg("--").args(paths);
    run_git_success(command, "git add")
}

fn git_has_staged_changes(
    vault: &Path,
    child_env: &ChildEnv,
    paths: &[PathBuf],
) -> Result<bool, GitFailure> {
    let mut command = ob::git_command(vault, child_env);
    command
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .arg("--exit-code")
        .arg("--")
        .args(paths);
    let output = command
        .output()
        .map_err(|error| spawn_failure("git diff", &error))?;

    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(report_git_failure("git diff --cached", &output)),
    }
}

fn commit_git_paths(
    vault: &Path,
    child_env: &ChildEnv,
    message: &str,
    paths: &[PathBuf],
) -> Result<(), GitFailure> {
    let mut command = ob::git_command(vault, child_env);
    command
        .arg("commit")
        .arg("-m")
        .arg(message)
        .arg("--")
        .args(paths);
    run_git_success(command, "git commit")
}

fn push_git(vault: &Path, child_env: &ChildEnv) -> Result<(), GitFailure> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("push");
    run_git_success(command, "git push")
}

fn run_git_success(
    mut command: Command,
    action: &'static str,
) -> Result<(), GitFailure> {
    let output = command
        .output()
        .map_err(|error| spawn_failure(action, &error))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(report_git_failure(action, &output))
    }
}

fn spawn_failure(action: &'static str, error: &io::Error) -> GitFailure {
    let message = format!("failed to run {action}: {error}");
    eprintln!("{COMMAND_NAME}: {message}");
    GitFailure {
        step: action,
        message,
        exit_code: 1,
    }
}

/// Echo a failed subprocess's output to stderr and describe the failure.
fn report_git_failure(action: &'static str, output: &Output) -> GitFailure {
    write_stderr_output(&merged_output(output));
    let exit_code = bob_env::exit_code(output.status);
    let message = format!("{action} failed with exit code {exit_code}");
    eprintln!("{COMMAND_NAME}: {message}");
    GitFailure {
        step: action,
        message,
        exit_code,
    }
}

pub(super) fn collect_done_commit_message() -> String {
    format!(
        "{} {}",
        super::COMMAND_NAME,
        bob_env::current_datetime().format("%Y-%m-%d")
    )
}
