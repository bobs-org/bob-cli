use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Component, Path, PathBuf},
    process::{self, Command, Output, Stdio},
};

use super::{
    env as bob_env,
    ob::{self, ChildEnv},
};

const COMMAND_NAME: &str = "bob collect-done";
pub(crate) const DEFAULT_THRESHOLD: usize = 10;
const ARCHIVE_TYPE_LINE: &str = "type: \"[[done]]\"";
const DONE_TASKS_KEY: &str = "done_tasks:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Args {
    threshold: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionPlan {
    scanned_files: usize,
    files: Vec<FilePlan>,
}

impl CollectionPlan {
    fn total_task_count(&self) -> usize {
        self.files.iter().map(|file| file.task_count).sum()
    }

    fn task_move_file_count(&self) -> usize {
        self.files.iter().filter(|file| file.task_count > 0).count()
    }

    fn source_file_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.writes_source())
            .count()
    }

    fn source_metadata_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.source_metadata_updated)
            .count()
    }

    fn archive_metadata_update_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.archive_metadata_updated)
            .count()
    }

    fn planned_bytes(&self) -> usize {
        self.files
            .iter()
            .map(|file| {
                let source_bytes = if file.writes_source() {
                    file.source_contents.len()
                } else {
                    0
                };
                let archive_bytes = file
                    .archive_contents
                    .as_ref()
                    .map(String::len)
                    .unwrap_or(0);
                source_bytes + archive_bytes
            })
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePlan {
    relative_source_path: PathBuf,
    relative_archive_path: PathBuf,
    task_count: usize,
    source_contents: String,
    archive_contents: Option<String>,
    source_metadata_updated: bool,
    archive_metadata_updated: bool,
}

impl FilePlan {
    fn writes_source(&self) -> bool {
        self.task_count > 0 || self.source_metadata_updated
    }

    fn writes_archive(&self) -> bool {
        self.archive_contents.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveWrite {
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitState {
    Worktree {
        touched_paths: Vec<PathBuf>,
        commit_message: String,
    },
    Skipped {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitPrepareError {
    DirtyCandidateFiles(Vec<String>),
    Command(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitDetection {
    Worktree,
    NotWorktree,
    MissingGit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Transform {
    task_count: usize,
    source_contents: String,
    archive_append: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskLine {
    indent: usize,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    match parse_args(args) {
        ParseResult::Run(args) => {
            let child_env = ob::child_env();
            run_collection(args.threshold, &child_env)
        }
        ParseResult::Help => {
            print_help();
            0
        }
        ParseResult::Error(message) => {
            eprintln!("{COMMAND_NAME}: {message}");
            eprintln!("Try '{COMMAND_NAME} --help' for more information.");
            2
        }
    }
}

/// Run the archive collection against the vault and commit/push the result.
///
/// This does **not** run `ob sync`; under `cronjob` the shared sync gate runs
/// once up front. The standalone `run()` wraps this directly.
pub(crate) fn run_collection(threshold: usize, child_env: &ChildEnv) -> i32 {
    let vault = bob_env::bob_dir();

    println!("Collect done tasks");
    println!("vault: {}", vault.display());
    println!("threshold: {threshold}");

    let plan = match build_collection_plan(&vault, threshold) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!(
                "{COMMAND_NAME}: failed to scan {}: {error}",
                vault.display()
            );
            return 1;
        }
    };

    println!("scan:");
    println!("  markdown files: {}", plan.scanned_files);
    println!("  files meeting threshold: {}", plan.task_move_file_count());
    println!("  task blocks: {}", plan.total_task_count());
    println!(
        "  source done_tasks updates: {}",
        plan.source_metadata_update_count()
    );
    println!(
        "  archive metadata repairs: {}",
        plan.archive_metadata_update_count()
    );
    println!("  planned bytes: {}", plan.planned_bytes());

    if plan.files.is_empty() {
        println!("moves:");
        println!("  none");
        println!("git:");
        println!("  skipped: no vault changes");
        println!("summary:");
        println!("  no task blocks met the threshold; no vault changes made.");
        return 0;
    }

    let git_state = match prepare_git(&vault, child_env, &plan) {
        Ok(git_state) => git_state,
        Err(GitPrepareError::DirtyCandidateFiles(paths)) => {
            println!("git:");
            println!("  detected: git worktree");
            println!("  refusing: pre-existing changes in candidate files");
            eprintln!(
                "{COMMAND_NAME}: refusing to modify candidate files with pre-existing git changes:"
            );
            for path in paths {
                eprintln!("  {path}");
            }
            return 1;
        }
        Err(GitPrepareError::Command(exit_code)) => return exit_code,
    };

    println!("moves:");
    let mut archives_created = 0;
    let mut archives_updated = 0;
    for file in &plan.files {
        if file.writes_archive() {
            if file.task_count > 0 {
                println!(
                    "  {} -> {} ({} task blocks)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display(),
                    file.task_count
                );
            } else if file.source_metadata_updated {
                println!(
                    "  {} -> {} (source/archive metadata)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display()
                );
            } else {
                println!(
                    "  {} -> {} (archive metadata)",
                    file.relative_source_path.display(),
                    file.relative_archive_path.display()
                );
            }
        } else {
            println!(
                "  {} -> {} (done_tasks metadata)",
                file.relative_source_path.display(),
                file.relative_archive_path.display()
            );
        }
        match apply_file_plan(&vault, file) {
            Ok(Some(ArchiveWrite::Created)) => archives_created += 1,
            Ok(Some(ArchiveWrite::Updated)) => archives_updated += 1,
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "{COMMAND_NAME}: failed to write vault changes: {error}"
                );
                return 1;
            }
        }
    }
    println!("git:");
    if let Err(exit_code) = finish_git(&vault, child_env, &git_state) {
        return exit_code;
    }
    println!("summary:");
    println!("  moved task blocks: {}", plan.total_task_count());
    println!(
        "  source files updated: {}",
        plan.source_file_update_count()
    );
    println!(
        "  source done_tasks updated: {}",
        plan.source_metadata_update_count()
    );
    println!(
        "  archive metadata repaired: {}",
        plan.archive_metadata_update_count()
    );
    println!("  archive files created: {archives_created}");
    println!("  archive files updated: {archives_updated}");
    0
}

fn merged_output(output: &Output) -> String {
    let mut merged = String::new();
    merged.push_str(&String::from_utf8_lossy(&output.stdout));
    merged.push_str(&String::from_utf8_lossy(&output.stderr));
    merged
}

fn write_stderr_output(output: &str) {
    if !output.is_empty() {
        eprint!("{output}");
    }
}

fn apply_file_plan(
    vault: &Path,
    file: &FilePlan,
) -> io::Result<Option<ArchiveWrite>> {
    let source_path = vault.join(&file.relative_source_path);
    let archive_path = vault.join(&file.relative_archive_path);
    let archive_write =
        if let Some(archive_contents) = file.archive_contents.as_deref() {
            let archive_write = if archive_path.is_file() {
                ArchiveWrite::Updated
            } else {
                ArchiveWrite::Created
            };
            atomic_write(&archive_path, archive_contents)?;
            Some(archive_write)
        } else {
            None
        };

    if file.writes_source() {
        atomic_write(&source_path, &file.source_contents)?;
    }

    Ok(archive_write)
}

fn prepare_git(
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
    let dirty_paths = dirty_candidate_paths(vault, child_env, &touched_paths)?;
    if !dirty_paths.is_empty() {
        return Err(GitPrepareError::DirtyCandidateFiles(dirty_paths));
    }

    Ok(GitState::Worktree {
        touched_paths,
        commit_message: collect_done_commit_message(),
    })
}

fn detect_git_worktree(
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

fn touched_git_paths(plan: &CollectionPlan) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for file in &plan.files {
        if file.writes_source() {
            paths.insert(file.relative_source_path.clone());
        }
        if file.writes_archive() {
            paths.insert(file.relative_archive_path.clone());
        }
    }
    paths.into_iter().collect()
}

fn dirty_candidate_paths(
    vault: &Path,
    child_env: &ChildEnv,
    touched_paths: &[PathBuf],
) -> Result<Vec<String>, GitPrepareError> {
    let mut command = ob::git_command(vault, child_env);
    command
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--untracked-files=all")
        .arg("--")
        .args(touched_paths);
    let output = command.output().map_err(|error| {
        eprintln!("{COMMAND_NAME}: failed to run git status: {error}");
        GitPrepareError::Command(1)
    })?;

    if !output.status.success() {
        report_git_failure("git status", &output);
        return Err(GitPrepareError::Command(bob_env::exit_code(
            output.status,
        )));
    }

    let status = String::from_utf8_lossy(&output.stdout);
    Ok(status
        .lines()
        .map(|line| line.get(3..).unwrap_or(line).to_string())
        .collect())
}

fn finish_git(
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

fn stage_git_paths(
    vault: &Path,
    child_env: &ChildEnv,
    paths: &[PathBuf],
) -> Result<(), i32> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("add").arg("--").args(paths);
    run_git_success(command, "git add")
}

fn git_has_staged_changes(
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

fn commit_git_paths(
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

fn push_git(vault: &Path, child_env: &ChildEnv) -> Result<(), i32> {
    let mut command = ob::git_command(vault, child_env);
    command.arg("push");
    run_git_success(command, "git push")
}

fn run_git_success(mut command: Command, action: &str) -> Result<(), i32> {
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

fn report_git_failure(action: &str, output: &Output) {
    write_stderr_output(&merged_output(output));
    eprintln!(
        "{COMMAND_NAME}: {action} failed with exit code {}",
        bob_env::exit_code(output.status)
    );
}

fn collect_done_commit_message() -> String {
    format!(
        "bob collect-done {}",
        bob_env::current_datetime().format("%Y-%m-%d")
    )
}

fn read_optional_string(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fs_error("read", path, error)),
    }
}

fn archive_contents(
    existing_archive: Option<&str>,
    archive_append: &str,
    source_link: &str,
) -> String {
    let mut contents =
        archive_base_contents(existing_archive, archive_append, source_link);
    append_archive_blocks(&mut contents, archive_append);
    contents
}

fn archive_base_contents(
    existing_archive: Option<&str>,
    sample: &str,
    source_link: &str,
) -> String {
    match existing_archive {
        Some(contents) => ensure_archive_frontmatter(contents, source_link),
        None => archive_frontmatter(sample, source_link),
    }
}

fn ensure_archive_frontmatter(contents: &str, source_link: &str) -> String {
    let newline = preferred_line_ending(contents);
    let parent_line = archive_parent_frontmatter_line(source_link);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    if lines
        .first()
        .map(|line| is_frontmatter_marker(split_line_ending(line).0))
        != Some(true)
    {
        let mut with_frontmatter = archive_frontmatter(contents, source_link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    }

    let Some(closing_index) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            is_frontmatter_marker(split_line_ending(line).0).then_some(index)
        })
    else {
        let mut with_frontmatter = archive_frontmatter(contents, source_link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    };

    let mut result = String::with_capacity(contents.len() + 64);

    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            result.push_str(line);
        } else if index < closing_index {
            let (content, _) = split_line_ending(line);
            if !is_parent_frontmatter_line(content)
                && !is_type_frontmatter_line(content)
            {
                result.push_str(line);
            }
        } else if index == closing_index {
            result.push_str(&parent_line);
            result.push_str(newline);
            result.push_str(ARCHIVE_TYPE_LINE);
            result.push_str(newline);
            result.push_str(line);
        } else {
            result.push_str(line);
        }
    }

    result
}

fn archive_frontmatter(sample: &str, source_link: &str) -> String {
    let newline = preferred_line_ending(sample);
    let parent_line = archive_parent_frontmatter_line(source_link);
    format!(
        "---{newline}{parent_line}{newline}{ARCHIVE_TYPE_LINE}{newline}---{newline}{newline}"
    )
}

fn archive_parent_frontmatter_line(source_link: &str) -> String {
    format!("parent: \"{source_link}\"")
}

fn ensure_source_done_tasks_frontmatter(contents: &str, link: &str) -> String {
    let newline = preferred_line_ending(contents);
    let done_tasks_line = done_tasks_frontmatter_line(link);
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();

    if lines
        .first()
        .map(|line| is_frontmatter_marker(split_line_ending(line).0))
        != Some(true)
    {
        let mut with_frontmatter =
            source_done_tasks_frontmatter(contents, link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    }

    let Some(closing_index) =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            is_frontmatter_marker(split_line_ending(line).0).then_some(index)
        })
    else {
        let mut with_frontmatter =
            source_done_tasks_frontmatter(contents, link);
        with_frontmatter.push_str(contents);
        return with_frontmatter;
    };

    let mut result = String::with_capacity(contents.len() + 80);
    let mut done_tasks_written = false;

    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            result.push_str(line);
        } else if index < closing_index {
            let (content, ending) = split_line_ending(line);
            if is_done_tasks_frontmatter_line(content) {
                result.push_str(&done_tasks_line);
                result.push_str(if ending.is_empty() {
                    newline
                } else {
                    ending
                });
                done_tasks_written = true;
            } else {
                result.push_str(line);
            }
        } else if index == closing_index {
            if !done_tasks_written {
                result.push_str(&done_tasks_line);
                result.push_str(newline);
            }
            result.push_str(line);
        } else {
            result.push_str(line);
        }
    }

    result
}

fn source_done_tasks_frontmatter(sample: &str, link: &str) -> String {
    let newline = preferred_line_ending(sample);
    let done_tasks_line = done_tasks_frontmatter_line(link);
    format!("---{newline}{done_tasks_line}{newline}---{newline}{newline}")
}

fn done_tasks_frontmatter_line(link: &str) -> String {
    format!("{DONE_TASKS_KEY} \"{link}\"")
}

fn append_archive_blocks(contents: &mut String, archive_append: &str) {
    if archive_append.is_empty() {
        return;
    }

    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(archive_append);
}

fn preferred_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn is_frontmatter_marker(content: &str) -> bool {
    content.trim() == "---"
}

fn is_parent_frontmatter_line(content: &str) -> bool {
    content.trim_start().starts_with("parent:")
}

fn is_type_frontmatter_line(content: &str) -> bool {
    content.trim_start().starts_with("type:")
}

fn is_done_tasks_frontmatter_line(content: &str) -> bool {
    content.starts_with(DONE_TASKS_KEY)
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            fs_error("create parent directory", parent, error)
        })?;
    }

    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents)
        .map_err(|error| fs_error("write temporary file", &temp_path, error))?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        fs_error("install file", path, error)
    })?;
    Ok(())
}

fn temporary_write_path(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;

    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    Ok(path.with_file_name(temp_name))
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{action} {}: {error}", path.display()),
    )
}

fn build_collection_plan(
    vault: &Path,
    threshold: usize,
) -> io::Result<CollectionPlan> {
    let markdown_files = markdown_files(vault)?;
    let mut files = Vec::new();

    for path in &markdown_files {
        let contents = fs::read_to_string(path)?;
        let relative_source_path = path
            .strip_prefix(vault)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "source path {} is outside vault {}: {error}",
                        path.display(),
                        vault.display()
                    ),
                )
            })?
            .to_path_buf();
        let relative_archive_path =
            archive_relative_path(&relative_source_path)?;
        let transform = transform_markdown(&contents);
        let moves_tasks = transform.task_count >= threshold;
        let archive_path = vault.join(&relative_archive_path);
        let existing_archive = read_optional_string(&archive_path)?;
        let archive_exists = existing_archive.is_some();

        let source_base = if moves_tasks {
            transform.source_contents
        } else {
            contents
        };
        let (source_contents, source_metadata_updated) =
            if moves_tasks || archive_exists {
                let link = archive_wiki_link(&relative_archive_path)?;
                let linked_source =
                    ensure_source_done_tasks_frontmatter(&source_base, &link);
                let source_metadata_updated = linked_source != source_base;
                (linked_source, source_metadata_updated)
            } else {
                (source_base, false)
            };
        let task_count = if moves_tasks { transform.task_count } else { 0 };
        let archive_append = if moves_tasks {
            transform.archive_append
        } else {
            String::new()
        };
        let (archive_contents, archive_metadata_updated) =
            if moves_tasks || archive_exists {
                let source_link = source_wiki_link(&relative_source_path)?;
                let archive_base = archive_base_contents(
                    existing_archive.as_deref(),
                    &archive_append,
                    &source_link,
                );
                let archive_metadata_updated = existing_archive
                    .as_deref()
                    .map(|contents| archive_base != contents)
                    .unwrap_or(false);

                if moves_tasks || archive_metadata_updated {
                    (
                        Some(archive_contents(
                            existing_archive.as_deref(),
                            &archive_append,
                            &source_link,
                        )),
                        archive_metadata_updated,
                    )
                } else {
                    (None, archive_metadata_updated)
                }
            } else {
                (None, false)
            };

        if task_count == 0
            && !source_metadata_updated
            && !archive_metadata_updated
        {
            continue;
        }

        files.push(FilePlan {
            relative_source_path,
            relative_archive_path,
            task_count,
            source_contents,
            archive_contents,
            source_metadata_updated,
            archive_metadata_updated,
        });
    }

    Ok(CollectionPlan {
        scanned_files: markdown_files.len(),
        files,
    })
}

fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, vault, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_markdown_files(
    vault: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if should_skip_directory(vault, &path) {
                continue;
            }
            collect_markdown_files(vault, &path, files)?;
        } else if file_type.is_file() && is_markdown_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn should_skip_directory(vault: &Path, directory: &Path) -> bool {
    let relative = directory.strip_prefix(vault).unwrap_or(directory);
    relative.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name == OsStr::new("done")
                    || name == OsStr::new(".git")
                    || name == OsStr::new(".obsidian")
        )
    })
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn archive_relative_path(source_relative_path: &Path) -> io::Result<PathBuf> {
    let stem = source_relative_path.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "source path has no file stem: {}",
                source_relative_path.display()
            ),
        )
    })?;
    let mut archive_name = OsString::from(stem);
    archive_name.push("_done.md");

    let mut archive_path = PathBuf::from("done");
    if let Some(parent) = source_relative_path.parent()
        && !parent.as_os_str().is_empty()
    {
        archive_path.push(parent);
    }
    archive_path.push(archive_name);
    Ok(archive_path)
}

fn archive_wiki_link(archive_relative_path: &Path) -> io::Result<String> {
    vault_relative_wiki_link(archive_relative_path, "archive")
}

fn source_wiki_link(source_relative_path: &Path) -> io::Result<String> {
    vault_relative_wiki_link(source_relative_path, "source")
}

fn vault_relative_wiki_link(
    relative_path: &Path,
    path_kind: &str,
) -> io::Result<String> {
    let mut path_without_extension = relative_path.to_path_buf();
    path_without_extension.set_extension("");

    let mut components = Vec::new();
    for component in path_without_extension.components() {
        let Component::Normal(part) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{path_kind} path is not vault-relative: {}",
                    relative_path.display()
                ),
            ));
        };
        components.push(part.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{path_kind} path is not valid UTF-8: {}",
                    relative_path.display()
                ),
            )
        })?);
    }

    Ok(format!("[[{}]]", components.join("/")))
}

fn transform_markdown(contents: &str) -> Transform {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let mut source_contents = String::with_capacity(contents.len());
    let mut archive_append = String::new();
    let mut task_count = 0;
    let mut index = 0;

    while index < lines.len() {
        let Some(task_line) = collectible_task_line(lines[index]) else {
            source_contents.push_str(lines[index]);
            index += 1;
            continue;
        };

        let end = task_block_end(&lines, index, task_line.indent);
        task_count += 1;
        for line in &lines[index..end] {
            archive_append.push_str(line);
        }
        index = end;
    }

    Transform {
        task_count,
        source_contents,
        archive_append,
    }
}

fn task_block_end(lines: &[&str], start: usize, task_indent: usize) -> usize {
    let mut index = start + 1;
    let mut include_end = start + 1;
    let mut pending_blank = false;

    while index < lines.len() {
        let (content, _) = split_line_ending(lines[index]);
        if content.trim().is_empty() {
            pending_blank = true;
            index += 1;
            continue;
        }

        if leading_indent_len(content) > task_indent {
            pending_blank = false;
            index += 1;
            include_end = index;
            continue;
        }

        break;
    }

    if pending_blank && index == lines.len() {
        include_end = index;
    }

    include_end
}

fn collectible_task_line(line: &str) -> Option<TaskLine> {
    let (content, _) = split_line_ending(line);
    let indent = leading_indent_len(content);
    let rest = &content[indent..];
    let rest = strip_list_marker(rest)?.trim_start();
    let checkbox = rest.get(..3)?;

    if !matches!(checkbox, "[x]" | "[X]" | "[-]") {
        return None;
    }

    let after_checkbox = &rest[3..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return None;
    }

    has_task_tag(content).then_some(TaskLine { indent })
}

fn strip_list_marker(line: &str) -> Option<&str> {
    let first = line.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        let after_marker = &line[first.len_utf8()..];
        if after_marker.starts_with(char::is_whitespace) {
            return Some(after_marker);
        }
    }

    let digit_len = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return None;
    }

    let after_digits = &line[digit_len..];
    let marker = after_digits.chars().next()?;
    if !matches!(marker, '.' | ')') {
        return None;
    }

    let after_marker = &after_digits[marker.len_utf8()..];
    after_marker
        .starts_with(char::is_whitespace)
        .then_some(after_marker)
}

fn has_task_tag(text: &str) -> bool {
    let mut rest = text;
    while let Some(index) = rest.find("#task") {
        let after_index = index + "#task".len();
        let after = rest[after_index..].chars().next();
        if after.map(is_task_tag_boundary).unwrap_or(true) {
            return true;
        }
        rest = &rest[after_index..];
    }

    false
}

fn is_task_tag_boundary(character: char) -> bool {
    !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

fn leading_indent_len(line: &str) -> usize {
    line.char_indices()
        .find_map(|(index, character)| {
            (!matches!(character, ' ' | '\t')).then_some(index)
        })
        .unwrap_or(line.len())
}

fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        return (content, "\r\n");
    }
    if let Some(content) = line.strip_suffix('\n') {
        return (content, "\n");
    }
    (line, "")
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    let mut parsed = Args::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        let text = bob_env::os_to_string(&arg);
        match text.as_str() {
            "-h" | "--help" => return ParseResult::Help,
            "--threshold" => {
                let Some(value) = args.next() else {
                    return ParseResult::Error(
                        "option --threshold requires a value".to_string(),
                    );
                };
                parsed.threshold = match parse_threshold(&value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            "--" => {
                if let Some(extra) = args.next() {
                    return ParseResult::Error(format!(
                        "unexpected positional argument: {}",
                        bob_env::os_to_string(&extra)
                    ));
                }
            }
            _ if let Some(value) = text.strip_prefix("--threshold=") => {
                parsed.threshold = match parse_threshold_text(value) {
                    Ok(threshold) => threshold,
                    Err(message) => return ParseResult::Error(message),
                };
            }
            _ if text.starts_with('-') => {
                return ParseResult::Error(format!(
                    "unrecognized argument: {text}"
                ));
            }
            _ => {
                return ParseResult::Error(format!(
                    "unexpected positional argument: {text}"
                ));
            }
        }
    }

    ParseResult::Run(parsed)
}

enum ParseResult {
    Run(Args),
    Help,
    Error(String),
}

fn parse_threshold(value: &OsString) -> Result<usize, String> {
    parse_threshold_text(&bob_env::os_to_string(value))
}

fn parse_threshold_text(value: &str) -> Result<usize, String> {
    let threshold = value
        .parse::<usize>()
        .map_err(|_| format!("invalid --threshold value: {value}"))?;
    if threshold == 0 {
        return Err("--threshold must be at least 1".to_string());
    }

    Ok(threshold)
}

fn print_help() {
    println!(
        "\
usage: {COMMAND_NAME} [--threshold N]

Collect done and canceled Bob task blocks into archive notes, link sources,
and repair archive metadata.

options:
  -h, --help       show this help message and exit
  --threshold N    minimum completed/canceled task count per source note \
(default: {DEFAULT_THRESHOLD})"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        archive_contents, archive_relative_path, archive_wiki_link,
        build_collection_plan, ensure_source_done_tasks_frontmatter,
        parse_args, source_wiki_link, transform_markdown, Args, ParseResult,
        DEFAULT_THRESHOLD,
    };
    use std::{
        ffi::OsString,
        fs, io,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn parses_default_threshold() {
        match parse_args(vec![]) {
            ParseResult::Run(args) => {
                assert_eq!(args.threshold, DEFAULT_THRESHOLD);
            }
            _ => panic!("expected runnable args"),
        }
    }

    #[test]
    fn parses_threshold_option() {
        match parse_args(os_args(["--threshold", "15"])) {
            ParseResult::Run(args) => assert_eq!(args, Args { threshold: 15 }),
            _ => panic!("expected runnable args"),
        }
    }

    #[test]
    fn parses_threshold_equals_option() {
        match parse_args(os_args(["--threshold=3"])) {
            ParseResult::Run(args) => assert_eq!(args, Args { threshold: 3 }),
            _ => panic!("expected runnable args"),
        }
    }

    #[test]
    fn rejects_zero_threshold() {
        match parse_args(os_args(["--threshold", "0"])) {
            ParseResult::Error(message) => {
                assert!(message.contains("at least 1"));
            }
            _ => panic!("expected parse error"),
        }
    }

    #[test]
    fn recognizes_done_and_canceled_task_lines_only() {
        let transform = transform_markdown(
            "\
- [x] done #task
- [X] uppercase done #task
- [-] canceled #task
- [ ] active #task
- [/] in progress #task
- [x] done without task tag
- [x] not quite #tasks
",
        );

        assert_eq!(transform.task_count, 3);
        assert_eq!(
            transform.archive_append,
            "\
- [x] done #task
- [X] uppercase done #task
- [-] canceled #task
"
        );
        assert_eq!(
            transform.source_contents,
            "\
- [ ] active #task
- [/] in progress #task
- [x] done without task tag
- [x] not quite #tasks
"
        );
    }

    #[test]
    fn extracts_nested_blocks_and_continuations() {
        let transform = transform_markdown(include_str!(
            "../../tests/fixtures/collect_done/nested_blocks.md"
        ));

        assert_eq!(transform.task_count, 1);
        assert_eq!(
            transform.source_contents,
            include_str!(
                "../../tests/fixtures/collect_done/nested_blocks_source.md"
            )
        );
        assert_eq!(
            transform.archive_append,
            include_str!(
                "../../tests/fixtures/collect_done/nested_blocks_archive.md"
            )
        );
    }

    #[test]
    fn completed_child_moves_without_collecting_active_parent() {
        let transform = transform_markdown(
            "\
- [ ] active parent #task
  - [x] done child #task
    child continuation
  - [/] active child #task
",
        );

        assert_eq!(transform.task_count, 1);
        assert_eq!(
            transform.source_contents,
            "\
- [ ] active parent #task
  - [/] active child #task
"
        );
        assert_eq!(
            transform.archive_append,
            "  - [x] done child #task\n    child continuation\n"
        );
    }

    #[test]
    fn preserves_line_endings_in_source_and_archive() {
        let transform = transform_markdown(
            "- [x] done #task\r\n  detail\r\n- [ ] keep #task\r\n",
        );

        assert_eq!(transform.task_count, 1);
        assert_eq!(transform.source_contents, "- [ ] keep #task\r\n");
        assert_eq!(
            transform.archive_append,
            "- [x] done #task\r\n  detail\r\n"
        );
    }

    #[test]
    fn maps_source_notes_to_archive_notes() {
        assert_eq!(
            archive_relative_path(Path::new("obsidian.md")).unwrap(),
            PathBuf::from("done/obsidian_done.md")
        );
        assert_eq!(
            archive_relative_path(Path::new("foo/bar.md")).unwrap(),
            PathBuf::from("done/foo/bar_done.md")
        );
    }

    #[test]
    fn maps_archive_notes_to_obsidian_wiki_links() {
        assert_eq!(
            archive_wiki_link(Path::new("done/obsidian_done.md")).unwrap(),
            "[[done/obsidian_done]]"
        );
        assert_eq!(
            archive_wiki_link(Path::new("done/foo/bar_done.md")).unwrap(),
            "[[done/foo/bar_done]]"
        );
    }

    #[test]
    fn maps_source_notes_to_obsidian_wiki_links() {
        assert_eq!(
            source_wiki_link(Path::new("obsidian.md")).unwrap(),
            "[[obsidian]]"
        );
        assert_eq!(
            source_wiki_link(Path::new("foo/bar.md")).unwrap(),
            "[[foo/bar]]"
        );
    }

    #[test]
    fn creates_archive_frontmatter_for_new_archive_note() {
        let contents =
            archive_contents(None, "- [x] done #task\n", "[[obsidian]]");

        assert_eq!(
            contents,
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] done #task
"
        );
    }

    #[test]
    fn adds_archive_parent_to_existing_frontmatter() {
        let contents = archive_contents(
            Some(
                "\
---
title: Existing archive
---

- [x] old #task
",
            ),
            "- [-] canceled #task\n",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "\
---
title: Existing archive
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
- [-] canceled #task
"
        );
    }

    #[test]
    fn updates_existing_archive_parent_frontmatter() {
        let contents = archive_contents(
            Some(
                "\
---
parent: \"[[old]]\"
type: \"[[done]]\"
---
",
            ),
            "- [x] done #task\n",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---
- [x] done #task
"
        );
    }

    #[test]
    fn inserts_missing_archive_type_frontmatter() {
        let contents = archive_contents(
            Some(
                "\
---
parent: \"[[obsidian]]\"
---

- [x] old #task
",
            ),
            "",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
"
        );
    }

    #[test]
    fn replaces_stale_archive_type_frontmatter() {
        let contents = archive_contents(
            Some(
                "\
---
parent: \"[[obsidian]]\"
type: \"[[old]]\"
---
",
            ),
            "",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---
"
        );
    }

    #[test]
    fn leaves_correct_archive_frontmatter_unchanged() {
        let original = "\
---
title: Existing archive
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
";

        assert_eq!(
            archive_contents(Some(original), "", "[[obsidian]]"),
            original
        );
    }

    #[test]
    fn preserves_crlf_when_repairing_archive_frontmatter() {
        let contents = archive_contents(
            Some("---\r\nparent: \"[[done]]\"\r\n---\r\n\r\n"),
            "",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "---\r\nparent: \"[[obsidian]]\"\r\ntype: \"[[done]]\"\r\n---\r\n\r\n"
        );
    }

    #[test]
    fn creates_archive_frontmatter_with_nested_source_parent() {
        let contents =
            archive_contents(None, "- [x] done #task\n", "[[foo/bar]]");

        assert_eq!(
            contents,
            "\
---
parent: \"[[foo/bar]]\"
type: \"[[done]]\"
---

- [x] done #task
"
        );
    }

    #[test]
    fn prepends_archive_frontmatter_when_existing_note_has_none() {
        let contents = archive_contents(
            Some("# Archive\n"),
            "- [x] done #task\n",
            "[[obsidian]]",
        );

        assert_eq!(
            contents,
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

# Archive
- [x] done #task
"
        );
    }

    #[test]
    fn adds_done_tasks_to_existing_source_frontmatter() {
        let contents = ensure_source_done_tasks_frontmatter(
            "\
---
title: Project
---

# Project
",
            "[[done/project_done]]",
        );

        assert_eq!(
            contents,
            "\
---
title: Project
done_tasks: \"[[done/project_done]]\"
---

# Project
"
        );
    }

    #[test]
    fn creates_source_frontmatter_for_done_tasks() {
        let contents = ensure_source_done_tasks_frontmatter(
            "# Project\n",
            "[[done/project_done]]",
        );

        assert_eq!(
            contents,
            "\
---
done_tasks: \"[[done/project_done]]\"
---

# Project
"
        );
    }

    #[test]
    fn replaces_stale_done_tasks_frontmatter() {
        let contents = ensure_source_done_tasks_frontmatter(
            "\
---
done_tasks: \"[[done/old_done]]\"
title: Project
---
",
            "[[done/project_done]]",
        );

        assert_eq!(
            contents,
            "\
---
done_tasks: \"[[done/project_done]]\"
title: Project
---
"
        );
    }

    #[test]
    fn leaves_correct_done_tasks_frontmatter_unchanged() {
        let original = "\
---
done_tasks: \"[[done/project_done]]\"
title: Project
---

# Project
";

        assert_eq!(
            ensure_source_done_tasks_frontmatter(
                original,
                "[[done/project_done]]"
            ),
            original
        );
    }

    #[test]
    fn preserves_crlf_when_adding_done_tasks_frontmatter() {
        let contents = ensure_source_done_tasks_frontmatter(
            "---\r\ntitle: Project\r\n---\r\n\r\n# Project\r\n",
            "[[done/project_done]]",
        );

        assert_eq!(
            contents,
            "---\r\ntitle: Project\r\ndone_tasks: \"[[done/project_done]]\"\r\n---\r\n\r\n# Project\r\n"
        );
    }

    #[test]
    fn scans_markdown_files_with_exclusions_and_threshold() {
        let vault = TempDir::new("bob-cli-collect-done-vault");
        write_file(
            &vault.path().join("obsidian.md"),
            "\
- [x] one #task
- [-] two #task
",
        );
        write_file(&vault.path().join("foo/bar.md"), "- [x] nested #task\n");
        write_file(&vault.path().join("foo/not-markdown.txt"), "#task\n");
        write_file(&vault.path().join("done/old.md"), "- [x] archived #task\n");
        write_file(&vault.path().join(".git/config.md"), "- [x] git #task\n");
        write_file(
            &vault.path().join(".obsidian/settings.md"),
            "- [x] settings #task\n",
        );

        let plan = build_collection_plan(vault.path(), 2).expect("build plan");

        assert_eq!(plan.scanned_files, 2);
        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(file.relative_source_path, PathBuf::from("obsidian.md"));
        assert_eq!(
            file.relative_archive_path,
            PathBuf::from("done/obsidian_done.md")
        );
        assert_eq!(file.task_count, 2);
        assert_eq!(
            file.source_contents,
            "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

"
        );
        assert_eq!(
            file.archive_contents.as_deref(),
            Some(
                "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] one #task
- [-] two #task
"
            )
        );
        assert!(file.source_metadata_updated);
        assert!(!file.archive_metadata_updated);
    }

    #[test]
    fn task_moving_plan_writes_archive_with_nested_source_parent() {
        let vault = TempDir::new("bob-cli-collect-done-nested-parent");
        write_file(
            &vault.path().join("foo/bar.md"),
            "\
- [x] nested #task
- [ ] active #task
",
        );

        let plan = build_collection_plan(vault.path(), 1).expect("build plan");

        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(
            file.archive_contents.as_deref(),
            Some(
                "\
---
parent: \"[[foo/bar]]\"
type: \"[[done]]\"
---

- [x] nested #task
"
            )
        );
        assert!(!file.archive_metadata_updated);
    }

    #[test]
    fn includes_nested_path_note_when_it_meets_threshold() {
        let vault = TempDir::new("bob-cli-collect-done-nested-vault");
        write_file(&vault.path().join("foo/bar.md"), "- [x] nested #task\n");

        let plan = build_collection_plan(vault.path(), 1).expect("build plan");

        assert_eq!(plan.files.len(), 1);
        assert_eq!(
            plan.files[0].relative_source_path,
            PathBuf::from("foo/bar.md")
        );
        assert_eq!(
            plan.files[0].relative_archive_path,
            PathBuf::from("done/foo/bar_done.md")
        );
    }

    #[test]
    fn collecting_tasks_adds_done_tasks_to_source() {
        let vault = TempDir::new("bob-cli-collect-done-source-link");
        write_file(
            &vault.path().join("foo/bar.md"),
            "\
- [x] nested #task
- [ ] active #task
",
        );

        let plan = build_collection_plan(vault.path(), 1).expect("build plan");

        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(file.task_count, 1);
        assert!(file.source_metadata_updated);
        assert_eq!(
            file.source_contents,
            "\
---
done_tasks: \"[[done/foo/bar_done]]\"
---

- [ ] active #task
"
        );
    }

    #[test]
    fn existing_archive_creates_metadata_only_source_update() {
        let vault = TempDir::new("bob-cli-collect-done-backfill");
        write_file(
            &vault.path().join("obsidian.md"),
            "\
- [x] below threshold #task
- [ ] active #task
",
        );
        write_file(
            &vault.path().join("done/obsidian_done.md"),
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
",
        );

        let plan = build_collection_plan(vault.path(), 2).expect("build plan");

        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(file.task_count, 0);
        assert!(file.archive_contents.is_none());
        assert!(file.source_metadata_updated);
        assert!(!file.archive_metadata_updated);
        assert_eq!(
            file.source_contents,
            "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [x] below threshold #task
- [ ] active #task
"
        );
    }

    #[test]
    fn existing_archive_with_stale_metadata_creates_archive_only_plan() {
        let vault = TempDir::new("bob-cli-collect-done-archive-repair");
        write_file(
            &vault.path().join("obsidian.md"),
            "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
        );
        write_file(
            &vault.path().join("done/obsidian_done.md"),
            "\
---
parent: \"[[done]]\"
---

- [x] old #task
",
        );

        let plan = build_collection_plan(vault.path(), 1).expect("build plan");

        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(file.task_count, 0);
        assert!(!file.source_metadata_updated);
        assert!(file.archive_metadata_updated);
        assert!(!file.writes_source());
        assert_eq!(
            file.archive_contents.as_deref(),
            Some(
                "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
"
            )
        );
    }

    #[test]
    fn already_linked_source_with_existing_archive_is_not_planned() {
        let vault = TempDir::new("bob-cli-collect-done-already-linked");
        write_file(
            &vault.path().join("obsidian.md"),
            "\
---
done_tasks: \"[[done/obsidian_done]]\"
---

- [ ] active #task
",
        );
        write_file(
            &vault.path().join("done/obsidian_done.md"),
            "\
---
parent: \"[[obsidian]]\"
type: \"[[done]]\"
---

- [x] old #task
",
        );

        let plan = build_collection_plan(vault.path(), 1).expect("build plan");

        assert!(plan.files.is_empty());
    }

    #[test]
    fn missing_archive_without_threshold_tasks_is_not_planned() {
        let vault = TempDir::new("bob-cli-collect-done-missing-archive");
        write_file(
            &vault.path().join("obsidian.md"),
            "\
- [x] below threshold #task
- [ ] active #task
",
        );

        let plan = build_collection_plan(vault.path(), 2).expect("build plan");

        assert!(plan.files.is_empty());
    }

    fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
        args.into_iter().map(OsString::from).collect()
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!("create parent {}: {error}", parent.display())
            });
        }
        fs::write(path, contents).unwrap_or_else(|error| {
            panic!("write {}: {error}", path.display())
        });
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| {
                panic!("create temp dir {}: {error}", path.display())
            });
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = remove_dir_all_if_exists(&self.path) {
                eprintln!(
                    "failed to remove temp dir {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }

    fn remove_dir_all_if_exists(path: &Path) -> io::Result<()> {
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}
