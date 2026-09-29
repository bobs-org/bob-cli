//! Git worktree detection and commit lifecycle.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GitState {
    Worktree {
        touched_paths: Vec<PathBuf>,
        commit_message: String,
    },
    Skipped {
        message: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GitPrepareError {
    Command(i32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitDetection {
    Worktree,
    NotWorktree,
    MissingGit,
}
pub(super) fn prepare_git(
    vault: &Path,
    child_env: &ChildEnv,
    plan: &CollectionPlan,
) -> Result<GitState, GitPrepareError> {
    match detect_git_worktree(vault, child_env)? {
        GitDetection::Worktree => {}
        GitDetection::NotWorktree => {
            return Ok(GitState::Skipped {
                message: "warning: vault is not a git worktree; skipping commit and push"
                    .to_string(),
            });
        }
        GitDetection::MissingGit => {
            return Ok(GitState::Skipped {
                message:
                    "warning: git command not found; skipping commit and push"
                        .to_string(),
            });
        }
    }

    let touched_paths = touched_git_paths(plan);
    Ok(GitState::Worktree {
        touched_paths,
        commit_message: collect_done_commit_message(),
    })
}
pub(super) fn detect_git_worktree(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<GitDetection, GitPrepareError> {
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
        Err(error) => {
            eprintln!("{COMMAND_NAME}: failed to run git rev-parse: {error}");
            Err(GitPrepareError::Command(1))
        }
    }
}
pub(super) fn touched_git_paths(plan: &CollectionPlan) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for file in &plan.files {
        if file.writes_source() {
            paths.insert(file.relative_source_path.clone());
        }
        if file.writes_archive() {
            paths.insert(file.relative_archive_path.clone());
        }
    }
    for repair in &plan.link_repairs {
        paths.insert(repair.relative_path.clone());
    }
    paths.into_iter().collect()
}
pub(super) fn finish_git(
    vault: &Path,
    child_env: &ChildEnv,
    git_state: &GitState,
) -> Result<(), i32> {
    match git_state {
        GitState::Skipped { message } => {
            println!("  {message}");
            Ok(())
        }
        GitState::Worktree {
            touched_paths,
            commit_message,
        } => {
            println!("  detected: git worktree");
            stage_git_paths(vault, child_env, touched_paths)?;
            println!("  staged paths: {}", touched_paths.len());

            if !git_has_staged_changes(vault, child_env, touched_paths)? {
                println!("  skipped: no collection changes to commit");
                return Ok(());
            }

            commit_git_paths(vault, child_env, commit_message, touched_paths)?;
            println!("  committed: {commit_message}");
            push_git(vault, child_env)?;
            println!("  pushed");
            Ok(())
        }
    }
}
pub(super) fn stage_git_paths(
    vault: &Path,
    child_env: &ChildEnv,
    paths: &[PathBuf],
) -> Result<(), i32> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("add").arg("--").args(paths);
    run_git_success(command, "git add")
}
pub(super) fn git_has_staged_changes(
    vault: &Path,
    child_env: &ChildEnv,
    paths: &[PathBuf],
) -> Result<bool, i32> {
    let mut command = ob::git_command(vault, child_env);
    command
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .arg("--exit-code")
        .arg("--")
        .args(paths);
    let output = command.output().map_err(|error| {
        eprintln!("{COMMAND_NAME}: failed to run git diff: {error}");
        1
    })?;

    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => {
            report_git_failure("git diff --cached", &output);
            Err(bob_env::exit_code(output.status))
        }
    }
}
pub(super) fn commit_git_paths(
    vault: &Path,
    child_env: &ChildEnv,
    message: &str,
    paths: &[PathBuf],
) -> Result<(), i32> {
    let mut command = ob::git_command(vault, child_env);
    command
        .arg("commit")
        .arg("-m")
        .arg(message)
        .arg("--")
        .args(paths);
    run_git_success(command, "git commit")
}
pub(super) fn push_git(vault: &Path, child_env: &ChildEnv) -> Result<(), i32> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("push");
    run_git_success(command, "git push")
}
pub(super) fn run_git_success(
    mut command: Command,
    action: &str,
) -> Result<(), i32> {
    let output = command.output().map_err(|error| {
        eprintln!("{COMMAND_NAME}: failed to run {action}: {error}");
        1
    })?;

    if output.status.success() {
        Ok(())
    } else {
        report_git_failure(action, &output);
        Err(bob_env::exit_code(output.status))
    }
}
pub(super) fn report_git_failure(action: &str, output: &Output) {
    write_stderr_output(&merged_output(output));
    eprintln!(
        "{COMMAND_NAME}: {action} failed with exit code {}",
        bob_env::exit_code(output.status)
    );
}
pub(super) fn collect_done_commit_message() -> String {
    format!(
        "bob move-done-tasks {}",
        bob_env::current_datetime().format("%Y-%m-%d")
    )
}
