use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{self, Write},
    iter,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use chrono::{Local, SecondsFormat};
use clap::{
    builder::NonEmptyStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::{Deserialize, Serialize};

use super::{
    env as bob_env,
    ob::{self, ChildEnv},
    style::Styler,
    sync,
};

const COMMAND_NAME: &str = "bob vault-sync";
const REMOTE: &str = "origin";
const BRANCH: &str = "master";
const LARGE_FILE_WARNING_BYTES: u64 = 50 * 1024 * 1024;
const LARGE_FILE_LIMIT_BYTES: u64 = 95 * 1024 * 1024;
const MAX_PUSH_RETRIES: usize = 3;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let args = default_run_args(args);
    let matches = match build_cli().try_get_matches_from(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                );
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("run", sub_matches)) => {
            run_cycle(RunOptions::from_matches(sub_matches))
        }
        Some(("status", sub_matches)) => run_status(sub_matches),
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        None => 2,
    }
}

fn default_run_args(args: Vec<OsString>) -> Vec<OsString> {
    let Some(first) = args.first() else {
        return vec![OsString::from("run")];
    };
    if first == OsStr::new("-h")
        || first == OsStr::new("--help")
        || first == OsStr::new("run")
        || first == OsStr::new("status")
    {
        return args;
    }
    if first.to_string_lossy().starts_with('-') {
        return iter::once(OsString::from("run")).chain(args).collect();
    }
    args
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Reconcile the Bob vault through Git")
        .long_about(
            "Run one lock-protected Git reconcile cycle for the Bob vault, or \
             report the last recorded cycle status.",
        )
        .subcommand(run_command())
        .subcommand(status_command())
        .after_help(
            "Examples:\n  bob vault-sync\n  bob vault-sync run --dry-run\n  bob vault-sync status --json\n\n\
Environment:\n  BOB_DIR                    Bob vault root; defaults to ~/bob\n  \
BOB_VAULT_SYNC_LOCK_FILE   lock file; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n  \
BOB_VAULT_SYNC_STATE_FILE  status JSON path; defaults to ${XDG_STATE_HOME:-$HOME/.local/state}/bob-cli/vault-sync.json\n  \
NO_COLOR                   disable color even when stdout is a TTY",
        )
}

fn run_command() -> ClapCommand {
    ClapCommand::new("run")
        .about("Run one reconcile cycle")
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('n')
                .action(ArgAction::SetTrue)
                .help("Report the cycle without staging, committing, merging, or pushing"),
        )
        .arg(
            Arg::new("message")
                .long("message")
                .short('m')
                .value_name("MESSAGE")
                .value_parser(NonEmptyStringValueParser::new())
                .help("Override the generated Git commit message"),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .short('q')
                .action(ArgAction::SetTrue)
                .help("Suppress per-step logging; errors and conflicts still print"),
        )
}

fn status_command() -> ClapCommand {
    ClapCommand::new("status")
        .about("Report the last reconcile cycle")
        .arg(
            Arg::new("json")
                .long("json")
                .short('j')
                .action(ArgAction::SetTrue)
                .help("Print the machine-readable status record"),
        )
}

#[derive(Debug, Clone)]
struct RunOptions {
    dry_run: bool,
    message: Option<String>,
    quiet: bool,
}

impl RunOptions {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            dry_run: matches.get_flag("dry-run"),
            message: matches.get_one::<String>("message").cloned(),
            quiet: matches.get_flag("quiet"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StatusRecord {
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    local_sha: Option<String>,
    remote_sha: Option<String>,
    files_committed: usize,
    push_retries: usize,
    duration_ms: u64,
    conflicts: Vec<String>,
    interrupted_merge_recovered: bool,
    last_error: Option<String>,
}

fn run_cycle(options: RunOptions) -> i32 {
    let vault = bob_env::bob_dir();
    let state_file = state_file_path();
    let styler = Styler::detect();

    let _lock = match ob::acquire_lock_quiet_if_held() {
        Ok(lock) => lock,
        Err(code) => return code,
    };

    let child_env = ob::child_env();
    let started = Instant::now();
    let mut status = read_status_record(&state_file)
        .unwrap_or_default()
        .unwrap_or_default();
    status.last_attempt_at = Some(now_rfc3339());
    status.files_committed = 0;
    status.push_retries = 0;
    status.duration_ms = 0;
    status.conflicts.clear();
    status.interrupted_merge_recovered = false;
    status.last_error = None;

    let outcome =
        run_cycle_inner(&vault, &child_env, &options, &styler, &mut status);
    status.duration_ms = elapsed_ms(started);
    refresh_status_shas(&vault, &child_env, &mut status);

    match outcome {
        Ok(()) => {
            if options.dry_run {
                print_log(&options, "dry-run complete; status file unchanged");
                return 0;
            }
            status.last_success_at = status.last_attempt_at.clone();
            status.last_error = None;
            if let Err(error) = write_status_record(&state_file, &status) {
                eprintln!(
                    "{COMMAND_NAME}: failed to write status file {}: {error}",
                    state_file.display()
                );
                return 1;
            }
            print_log(
                &options,
                &format!("wrote status record {}", state_file.display()),
            );
            0
        }
        Err(error) => {
            status.last_error = Some(error.message.clone());
            if !options.dry_run
                && let Err(write_error) =
                    write_status_record(&state_file, &status)
            {
                eprintln!(
                    "{COMMAND_NAME}: failed to write status file {}: {write_error}",
                    state_file.display()
                );
            }
            eprintln!("{}: {}", styler.red("error"), error.message);
            error.code
        }
    }
}

fn run_cycle_inner(
    vault: &Path,
    child_env: &ChildEnv,
    options: &RunOptions,
    styler: &Styler,
    status: &mut StatusRecord,
) -> Result<(), CycleError> {
    sync::verify_bob_worktree(vault, child_env).map_err(CycleError::fatal)?;
    print_log(
        options,
        &format!("reconciling Bob vault at {}", vault.display()),
    );

    status.interrupted_merge_recovered =
        recover_interrupted_operation(vault, child_env, options, styler)?;

    let changed_paths = working_tree_status(vault, child_env)?;
    if changed_paths.is_empty() {
        print_log(options, "working tree clean; skipping git add -A");
    } else {
        preflight_large_files(vault, &changed_paths, styler)?;
        if options.dry_run {
            print_log(
                options,
                &format!("would stage {} changed path(s)", changed_paths.len()),
            );
        } else {
            print_log(
                options,
                &format!("staging {} changed path(s)", changed_paths.len()),
            );
            git_success(vault, child_env, ["add", "-A", "."])?;
        }
    }

    if options.dry_run {
        report_dry_run_remote_plan(vault, child_env, options)?;
        return Ok(());
    }

    if git_diff_cached(vault, child_env)? {
        let paths = cached_diff_paths(vault, child_env)?;
        let name_status = cached_diff_name_status(vault, child_env)?;
        let message = options
            .message
            .clone()
            .unwrap_or_else(|| generated_commit_message(&paths, &name_status));
        print_log(
            options,
            &format!("committing {} staged file(s)", paths.len()),
        );
        git_commit(vault, child_env, &message)?;
        status.files_committed = paths.len();
    } else {
        print_log(options, "no staged changes to commit");
    }

    for attempt in 0..=MAX_PUSH_RETRIES {
        fetch_if_needed(vault, child_env, options)?;
        reconcile_origin_master(vault, child_env, options, styler, status)?;

        match push_origin_master(vault, child_env) {
            Ok(()) => {
                print_log(options, "pushed vault state");
                return Ok(());
            }
            Err(error)
                if error.is_non_fast_forward()
                    && attempt < MAX_PUSH_RETRIES =>
            {
                status.push_retries += 1;
                warn(
                    styler,
                    &format!(
                        "push rejected by a newer remote; retrying ({}/{MAX_PUSH_RETRIES})",
                        attempt + 1
                    ),
                );
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }

    Err(CycleError::fatal("push retry limit exceeded"))
}

fn recover_interrupted_operation(
    vault: &Path,
    child_env: &ChildEnv,
    options: &RunOptions,
    styler: &Styler,
) -> Result<bool, CycleError> {
    let mut recovered = false;

    if git_path_exists(vault, child_env, "MERGE_HEAD")? {
        recovered = true;
        if options.dry_run {
            warn(styler, "would abort interrupted merge before reconciling");
        } else {
            warn(styler, "aborting interrupted merge before reconciling");
            git_success(vault, child_env, ["merge", "--abort"])?;
        }
    }
    if git_path_exists(vault, child_env, "rebase-merge")?
        || git_path_exists(vault, child_env, "rebase-apply")?
    {
        recovered = true;
        if options.dry_run {
            warn(styler, "would abort interrupted rebase before reconciling");
        } else {
            warn(styler, "aborting interrupted rebase before reconciling");
            git_success(vault, child_env, ["rebase", "--abort"])?;
        }
    }
    if git_path_exists(vault, child_env, "CHERRY_PICK_HEAD")? {
        recovered = true;
        if options.dry_run {
            warn(
                styler,
                "would abort interrupted cherry-pick before reconciling",
            );
        } else {
            warn(
                styler,
                "aborting interrupted cherry-pick before reconciling",
            );
            git_success(vault, child_env, ["cherry-pick", "--abort"])?;
        }
    }

    Ok(recovered)
}

fn working_tree_status(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Vec<StatusEntry>, CycleError> {
    git_status_entries(
        vault,
        child_env,
        ["status", "--porcelain=v1", "-z", "-uall"],
    )
}

fn git_status_entries<const N: usize>(
    vault: &Path,
    child_env: &ChildEnv,
    args: [&str; N],
) -> Result<Vec<StatusEntry>, CycleError> {
    let output = git_output(vault, child_env, args)?;
    if !output.status.success() {
        return Err(CycleError::from_output("git status", output));
    }
    parse_status_entries(&output.stdout)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEntry {
    code: String,
    path: PathBuf,
}

fn parse_status_entries(bytes: &[u8]) -> Result<Vec<StatusEntry>, CycleError> {
    let mut entries = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes.len().saturating_sub(index) < 4 {
            return Err(CycleError::fatal("malformed git status output"));
        }
        let code =
            String::from_utf8_lossy(&bytes[index..index + 2]).into_owned();
        if bytes[index + 2] != b' ' {
            return Err(CycleError::fatal("malformed git status output"));
        }
        index += 3;

        let Some(path_end) = bytes[index..].iter().position(|byte| *byte == 0)
        else {
            return Err(CycleError::fatal("malformed git status output"));
        };
        let path = PathBuf::from(
            String::from_utf8_lossy(&bytes[index..index + path_end])
                .into_owned(),
        );
        index += path_end + 1;

        if code.starts_with('R') || code.starts_with('C') {
            let Some(old_path_end) =
                bytes[index..].iter().position(|byte| *byte == 0)
            else {
                return Err(CycleError::fatal("malformed git status output"));
            };
            index += old_path_end + 1;
        }

        entries.push(StatusEntry { code, path });
    }
    Ok(entries)
}

fn preflight_large_files(
    vault: &Path,
    entries: &[StatusEntry],
    styler: &Styler,
) -> Result<(), CycleError> {
    let mut paths = BTreeSet::new();
    for entry in entries {
        if entry.code == " D" || entry.code == "D " || entry.code == "DD" {
            continue;
        }
        paths.insert(entry.path.clone());
    }

    for path in paths {
        let full_path = vault.join(&path);
        let Ok(metadata) = fs::metadata(&full_path) else {
            continue;
        };
        if metadata.is_dir() {
            preflight_directory(vault, &full_path, styler)?;
        } else if metadata.is_file() {
            preflight_one_file(&path, metadata.len(), styler)?;
        }
    }

    Ok(())
}

fn preflight_directory(
    vault: &Path,
    directory: &Path,
    styler: &Styler,
) -> Result<(), CycleError> {
    for entry in fs::read_dir(directory).map_err(|error| {
        CycleError::fatal(format!(
            "failed to read {} during size preflight: {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CycleError::fatal(format!(
                "failed to read {} during size preflight: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            CycleError::fatal(format!(
                "failed to stat {} during size preflight: {error}",
                path.display()
            ))
        })?;
        if metadata.is_dir() {
            preflight_directory(vault, &path, styler)?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(vault).unwrap_or(&path);
            preflight_one_file(relative, metadata.len(), styler)?;
        }
    }
    Ok(())
}

fn preflight_one_file(
    relative_path: &Path,
    size: u64,
    styler: &Styler,
) -> Result<(), CycleError> {
    if size >= LARGE_FILE_LIMIT_BYTES {
        return Err(CycleError::fatal(format!(
            "refusing to stage {} ({}); GitHub rejects files at 100 MiB",
            display_path(relative_path),
            human_bytes(size)
        )));
    }
    if size >= LARGE_FILE_WARNING_BYTES {
        warn(
            styler,
            &format!(
                "{} is large ({}) and will be staged",
                display_path(relative_path),
                human_bytes(size)
            ),
        );
    }
    Ok(())
}

fn report_dry_run_remote_plan(
    vault: &Path,
    child_env: &ChildEnv,
    options: &RunOptions,
) -> Result<(), CycleError> {
    let remote_sha = ls_remote_master(vault, child_env)?;
    let cached_sha = cached_origin_master(vault, child_env)?;
    match (remote_sha.as_deref(), cached_sha.as_deref()) {
        (Some(remote), Some(cached)) if remote == cached => {
            print_log(options, "remote master matches cached origin/master");
        }
        (Some(_), Some(_)) => {
            print_log(options, "would fetch and reconcile newer remote master");
        }
        (Some(_), None) => {
            print_log(options, "would fetch remote master");
        }
        (None, _) => {
            print_log(
                options,
                "remote master is absent; would push local HEAD",
            );
        }
    }
    print_log(options, "would push after reconcile");
    Ok(())
}

fn fetch_if_needed(
    vault: &Path,
    child_env: &ChildEnv,
    options: &RunOptions,
) -> Result<(), CycleError> {
    let remote_sha = ls_remote_master(vault, child_env)?;
    let Some(remote_sha) = remote_sha else {
        print_log(options, "remote master absent; skipping fetch");
        return Ok(());
    };

    if cached_origin_master(vault, child_env)?.as_deref()
        == Some(remote_sha.as_str())
    {
        print_log(options, "cached origin/master is current; skipping fetch");
        return Ok(());
    }

    print_log(options, "fetching origin/master");
    git_success(vault, child_env, ["fetch", "--no-tags", REMOTE, BRANCH])
}

fn reconcile_origin_master(
    vault: &Path,
    child_env: &ChildEnv,
    options: &RunOptions,
    styler: &Styler,
    status: &mut StatusRecord,
) -> Result<(), CycleError> {
    let Some(remote_sha) = cached_origin_master(vault, child_env)? else {
        print_log(options, "no cached origin/master; skipping merge");
        return Ok(());
    };
    let Some(local_sha) = rev_parse(vault, child_env, "HEAD")? else {
        print_log(options, "local HEAD absent; skipping merge");
        return Ok(());
    };

    if local_sha == remote_sha {
        print_log(options, "local HEAD already matches origin/master");
        return Ok(());
    }

    if is_ancestor(vault, child_env, "HEAD", "refs/remotes/origin/master")? {
        print_log(options, "fast-forwarding to origin/master");
        return git_success(
            vault,
            child_env,
            ["merge", "--ff-only", "refs/remotes/origin/master"],
        );
    }

    print_log(options, "merging origin/master");
    match git_success(
        vault,
        child_env,
        ["merge", "--no-edit", "refs/remotes/origin/master"],
    ) {
        Ok(()) => Ok(()),
        Err(error) => {
            if conflict_entries(vault, child_env)?.is_empty() {
                return Err(error);
            }
            resolve_conflicts(vault, child_env, styler, status)?;
            print_log(options, "committing resolved merge");
            git_success(vault, child_env, ["commit", "--no-edit"])
        }
    }
}

fn conflict_entries(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Vec<StatusEntry>, CycleError> {
    Ok(git_status_entries(
        vault,
        child_env,
        ["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )?
    .into_iter()
    .filter(|entry| is_unmerged_code(&entry.code))
    .collect())
}

fn is_unmerged_code(code: &str) -> bool {
    matches!(code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
}

fn resolve_conflicts(
    vault: &Path,
    child_env: &ChildEnv,
    styler: &Styler,
    status: &mut StatusRecord,
) -> Result<(), CycleError> {
    let conflicts = conflict_entries(vault, child_env)?;
    let stages = unmerged_stages(vault, child_env)?;
    if conflicts.is_empty() {
        return Err(CycleError::fatal(
            "merge failed but git reported no conflicted paths",
        ));
    }

    let host = hostname_slug();
    let timestamp = Local::now().format("%Y-%m-%dT%H%M%S%z").to_string();
    let mut note_lines = Vec::new();
    let mut paths_to_add = BTreeSet::new();

    for conflict in conflicts {
        let path_stages = stages.get(&conflict.path).ok_or_else(|| {
            CycleError::fatal(format!(
                "missing index stages for conflicted path {}",
                display_path(&conflict.path)
            ))
        })?;
        let path_display = display_path(&conflict.path);

        match conflict.code.as_str() {
            "UU" | "AA" => {
                let ours = path_stages.stage(2).ok_or_else(|| {
                    CycleError::fatal(format!(
                        "missing local stage for conflicted path {path_display}"
                    ))
                })?;
                let conflict_copy = write_conflict_copy(
                    vault,
                    child_env,
                    &conflict.path,
                    ours,
                    &host,
                    &timestamp,
                )?;
                git_success_os(
                    vault,
                    child_env,
                    &[
                        OsStr::new("checkout"),
                        OsStr::new("--theirs"),
                        OsStr::new("--"),
                    ],
                    &conflict.path,
                )?;
                paths_to_add.insert(conflict.path.clone());
                paths_to_add.insert(conflict_copy.clone());
                status.conflicts.push(display_path(&conflict_copy));
                note_lines.push(format!(
                    "- {timestamp} {path_display} -> {} ({})",
                    display_path(&conflict_copy),
                    conflict_kind(&conflict)
                ));
                warn(
                    styler,
                    &format!(
                        "resolved {path_display} with remote version; quarantined local copy at {}",
                        display_path(&conflict_copy)
                    ),
                );
            }
            "UD" | "AU" => {
                git_success_os(
                    vault,
                    child_env,
                    &[
                        OsStr::new("checkout"),
                        OsStr::new("--ours"),
                        OsStr::new("--"),
                    ],
                    &conflict.path,
                )?;
                paths_to_add.insert(conflict.path.clone());
                note_lines.push(format!(
                    "- {timestamp} {path_display} kept local file after delete/modify conflict"
                ));
                warn(
                    styler,
                    &format!("kept local file for delete/modify conflict at {path_display}"),
                );
            }
            "DU" | "UA" => {
                git_success_os(
                    vault,
                    child_env,
                    &[
                        OsStr::new("checkout"),
                        OsStr::new("--theirs"),
                        OsStr::new("--"),
                    ],
                    &conflict.path,
                )?;
                paths_to_add.insert(conflict.path.clone());
                note_lines.push(format!(
                    "- {timestamp} {path_display} kept remote file after delete/modify conflict"
                ));
                warn(
                    styler,
                    &format!("kept remote file for delete/modify conflict at {path_display}"),
                );
            }
            other => {
                let message = format!(
                    "conflict_state: unhandled conflict {other} at {path_display}"
                );
                let _ = git_success(vault, child_env, ["merge", "--abort"]);
                run_notify(child_env, styler);
                return Err(CycleError::fatal(message));
            }
        }
    }

    let conflict_log = PathBuf::from("_conflicts").join("sync_conflicts.md");
    append_conflict_log(vault, &conflict_log, &note_lines)?;
    paths_to_add.insert(conflict_log);

    for path in paths_to_add {
        git_add_path(vault, child_env, &path)?;
    }
    run_notify(child_env, styler);
    Ok(())
}

fn conflict_kind(conflict: &StatusEntry) -> &'static str {
    match conflict.code.as_str() {
        "AA" => "both-added",
        "UU" if is_text_conflict_path(&conflict.path) => "both-modified text",
        "UU" => "both-modified binary",
        _ => "conflict",
    }
}

fn is_text_conflict_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "canvas" | "base"
            )
        })
}

fn write_conflict_copy(
    vault: &Path,
    child_env: &ChildEnv,
    original: &Path,
    object_id: &str,
    host: &str,
    timestamp: &str,
) -> Result<PathBuf, CycleError> {
    let contents = git_cat_file(vault, child_env, object_id)?;
    let relative = conflict_copy_path(original, host, timestamp);
    let full_path = vault.join(&relative);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CycleError::fatal(format!(
                "failed to create conflict directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let final_relative = unique_conflict_path(vault, relative);
    fs::write(vault.join(&final_relative), contents).map_err(|error| {
        CycleError::fatal(format!(
            "failed to write conflict copy {}: {error}",
            final_relative.display()
        ))
    })?;
    Ok(final_relative)
}

fn conflict_copy_path(original: &Path, host: &str, timestamp: &str) -> PathBuf {
    let mut relative = PathBuf::from("_conflicts");
    if let Some(parent) = original.parent() {
        relative.push(parent);
    }

    let file_name = original
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("conflict");
    let extension = original.extension().and_then(OsStr::to_str);
    let stem = original
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(file_name);
    let conflict_name = match extension {
        Some(extension) if !extension.is_empty() => {
            format!("{stem}.{host}-{timestamp}.{extension}")
        }
        _ => format!("{file_name}.{host}-{timestamp}"),
    };
    relative.push(conflict_name);
    relative
}

fn unique_conflict_path(vault: &Path, relative: PathBuf) -> PathBuf {
    if !vault.join(&relative).exists() {
        return relative;
    }

    let parent = relative.parent().map(Path::to_path_buf).unwrap_or_default();
    let file_name = relative
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("conflict");
    let extension = Path::new(file_name).extension().and_then(OsStr::to_str);
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(file_name);
    for index in 2.. {
        let mut candidate = parent.clone();
        let name = match extension {
            Some(extension) if !extension.is_empty() => {
                format!("{stem}.{index}.{extension}")
            }
            _ => format!("{stem}.{index}"),
        };
        candidate.push(name);
        if !vault.join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("unbounded conflict-copy suffix search must return")
}

fn append_conflict_log(
    vault: &Path,
    relative_path: &Path,
    lines: &[String],
) -> Result<(), CycleError> {
    if lines.is_empty() {
        return Ok(());
    }
    let full_path = vault.join(relative_path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CycleError::fatal(format!(
                "failed to create conflict log directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&full_path)
        .map_err(|error| {
            CycleError::fatal(format!(
                "failed to open conflict log {}: {error}",
                full_path.display()
            ))
        })?;
    for line in lines {
        writeln!(file, "{line}").map_err(|error| {
            CycleError::fatal(format!(
                "failed to write conflict log {}: {error}",
                full_path.display()
            ))
        })?;
    }
    Ok(())
}

fn git_add_path(
    vault: &Path,
    child_env: &ChildEnv,
    relative_path: &Path,
) -> Result<(), CycleError> {
    git_success_os(
        vault,
        child_env,
        &[OsStr::new("add"), OsStr::new("--")],
        relative_path,
    )
}

fn push_origin_master(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<(), CycleError> {
    git_success(vault, child_env, ["push", REMOTE, "HEAD:master"])
}

fn git_diff_cached(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<bool, CycleError> {
    let output = git_output(
        vault,
        child_env,
        ["diff", "--cached", "--quiet", "--exit-code"],
    )?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(CycleError::from_output("git diff --cached", output)),
    }
}

fn cached_diff_paths(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Vec<String>, CycleError> {
    let output =
        git_output(vault, child_env, ["diff", "--cached", "--name-only"])?;
    if !output.status.success() {
        return Err(CycleError::from_output(
            "git diff --cached --name-only",
            output,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

fn cached_diff_name_status(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Vec<String>, CycleError> {
    let output =
        git_output(vault, child_env, ["diff", "--cached", "--name-status"])?;
    if !output.status.success() {
        return Err(CycleError::from_output(
            "git diff --cached --name-status",
            output,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

fn generated_commit_message(
    paths: &[String],
    name_status: &[String],
) -> String {
    let host = hostname_slug();
    let count = paths.len();
    let mut subject_paths = paths.iter().take(3).cloned().collect::<Vec<_>>();
    subject_paths.sort();
    let tail = count.saturating_sub(subject_paths.len());
    let plural = if count == 1 { "file" } else { "files" };
    let tail_text = if tail > 0 {
        format!(" (+{tail})")
    } else {
        String::new()
    };
    let subject = format!(
        "vault({host}): {count} {plural} - {}{tail_text}",
        subject_paths.join(", ")
    );

    let mut body = name_status.iter().take(30).cloned().collect::<Vec<_>>();
    let extra = name_status.len().saturating_sub(body.len());
    if extra > 0 {
        body.push(format!("... and {extra} more"));
    }

    if body.is_empty() {
        subject
    } else {
        format!("{subject}\n\n{}", body.join("\n"))
    }
}

fn git_commit(
    vault: &Path,
    child_env: &ChildEnv,
    message: &str,
) -> Result<(), CycleError> {
    let mut child = ob::git_command(vault, child_env)
        .arg("commit")
        .arg("-F")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            CycleError::fatal(format!("failed to run git commit: {error}"))
        })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| CycleError::fatal("failed to open git commit stdin"))?
        .write_all(message.as_bytes())
        .map_err(|error| {
            CycleError::fatal(format!(
                "failed to write git commit message: {error}"
            ))
        })?;
    let output = child.wait_with_output().map_err(|error| {
        CycleError::fatal(format!("failed to wait for git commit: {error}"))
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CycleError::from_output("git commit", output))
    }
}

fn ls_remote_master(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Option<String>, CycleError> {
    let output = git_output(vault, child_env, ["ls-remote", REMOTE, BRANCH])?;
    if !output.status.success() {
        return Err(CycleError::from_output("git ls-remote", output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .find_map(|line| line.split_whitespace().next())
        .filter(|sha| !sha.is_empty())
        .map(str::to_string))
}

fn cached_origin_master(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<Option<String>, CycleError> {
    rev_parse(vault, child_env, "refs/remotes/origin/master")
}

fn rev_parse(
    vault: &Path,
    child_env: &ChildEnv,
    revision: &str,
) -> Result<Option<String>, CycleError> {
    let output = git_output(
        vault,
        child_env,
        ["rev-parse", "--verify", "--quiet", revision],
    )?;
    match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(CycleError::from_output("git rev-parse", output)),
    }
}

fn is_ancestor(
    vault: &Path,
    child_env: &ChildEnv,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, CycleError> {
    let output = git_output(
        vault,
        child_env,
        ["merge-base", "--is-ancestor", ancestor, descendant],
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CycleError::from_output(
            "git merge-base --is-ancestor",
            output,
        )),
    }
}

fn git_path_exists(
    vault: &Path,
    child_env: &ChildEnv,
    path: &str,
) -> Result<bool, CycleError> {
    let output =
        git_output(vault, child_env, ["rev-parse", "--git-path", path])?;
    if !output.status.success() {
        return Err(CycleError::from_output(
            "git rev-parse --git-path",
            output,
        ));
    }
    let git_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(!git_path.is_empty() && vault.join(git_path).exists())
}

#[derive(Debug, Default)]
struct PathStages {
    stages: BTreeMap<u8, String>,
}

impl PathStages {
    fn stage(&self, stage: u8) -> Option<&str> {
        self.stages.get(&stage).map(String::as_str)
    }
}

fn unmerged_stages(
    vault: &Path,
    child_env: &ChildEnv,
) -> Result<BTreeMap<PathBuf, PathStages>, CycleError> {
    let output = git_output(vault, child_env, ["ls-files", "-u", "-z"])?;
    if !output.status.success() {
        return Err(CycleError::from_output("git ls-files -u", output));
    }

    let mut result = BTreeMap::<PathBuf, PathStages>::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        let Some((metadata, path)) = text.split_once('\t') else {
            return Err(CycleError::fatal("malformed git ls-files -u output"));
        };
        let mut parts = metadata.split_whitespace();
        let _mode = parts.next();
        let Some(object_id) = parts.next() else {
            return Err(CycleError::fatal("malformed git ls-files -u output"));
        };
        let Some(stage) =
            parts.next().and_then(|value| value.parse::<u8>().ok())
        else {
            return Err(CycleError::fatal("malformed git ls-files -u output"));
        };
        result
            .entry(PathBuf::from(path))
            .or_default()
            .stages
            .insert(stage, object_id.to_string());
    }
    Ok(result)
}

fn git_cat_file(
    vault: &Path,
    child_env: &ChildEnv,
    object_id: &str,
) -> Result<Vec<u8>, CycleError> {
    let output = git_output(vault, child_env, ["cat-file", "-p", object_id])?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(CycleError::from_output("git cat-file", output))
    }
}

fn git_success<const N: usize>(
    vault: &Path,
    child_env: &ChildEnv,
    args: [&str; N],
) -> Result<(), CycleError> {
    let command = format!("git {}", args.join(" "));
    let output = git_output(vault, child_env, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CycleError::from_output(&command, output))
    }
}

fn git_success_os(
    vault: &Path,
    child_env: &ChildEnv,
    prefix: &[&OsStr],
    path: &Path,
) -> Result<(), CycleError> {
    let mut command = ob::git_command(vault, child_env);
    command.args(prefix).arg(path);
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            CycleError::fatal(format!("failed to run git: {error}"))
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CycleError::from_output("git", output))
    }
}

fn git_output<const N: usize>(
    vault: &Path,
    child_env: &ChildEnv,
    args: [&str; N],
) -> Result<Output, CycleError> {
    ob::git_command(vault, child_env)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            CycleError::fatal(format!("failed to run git: {error}"))
        })
}

fn run_notify(child_env: &ChildEnv, styler: &Styler) {
    match Command::new("bob")
        .arg("notify")
        .envs(child_env.iter().cloned())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warn(
                styler,
                "bob notify command not found; conflict notification skipped",
            );
        }
        Err(error) => {
            warn(
                styler,
                &format!("failed to run bob notify after conflict resolution: {error}"),
            );
        }
    }
}

fn run_status(matches: &ArgMatches) -> i32 {
    let state_file = state_file_path();
    let status = match read_status_record(&state_file) {
        Ok(Some(status)) => status,
        Ok(None) => {
            eprintln!(
                "{COMMAND_NAME}: no status record found at {}",
                state_file.display()
            );
            return 1;
        }
        Err(error) => {
            eprintln!(
                "{COMMAND_NAME}: failed to read status file {}: {error}",
                state_file.display()
            );
            return 1;
        }
    };

    if matches.get_flag("json") {
        match serde_json::to_string_pretty(&status) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!(
                    "{COMMAND_NAME}: failed to encode status JSON: {error}"
                );
                return 1;
            }
        }
        return 0;
    }

    print_status_panel(&state_file, &status);
    0
}

fn print_status_panel(state_file: &Path, status: &StatusRecord) {
    let styler = Styler::detect();
    println!("{}", styler.cyan("bob vault-sync status"));
    println!("state_file: {}", state_file.display());
    println!(
        "last_attempt_at: {}",
        status.last_attempt_at.as_deref().unwrap_or("-")
    );
    println!(
        "last_success_at: {}",
        status.last_success_at.as_deref().unwrap_or("-")
    );
    println!(
        "local_sha: {}",
        status
            .local_sha
            .as_deref()
            .map(short_sha)
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "remote_sha: {}",
        status
            .remote_sha
            .as_deref()
            .map(short_sha)
            .unwrap_or_else(|| "-".to_string())
    );
    println!("files_committed: {}", status.files_committed);
    println!("push_retries: {}", status.push_retries);
    println!("duration_ms: {}", status.duration_ms);
    println!(
        "interrupted_merge_recovered: {}",
        status.interrupted_merge_recovered
    );
    if status.conflicts.is_empty() {
        println!("conflicts: none");
    } else {
        println!("conflicts:");
        for conflict in &status.conflicts {
            println!("  - {conflict}");
        }
    }
    match &status.last_error {
        Some(error) => println!("last_error: {}", styler.red(error)),
        None => println!("last_error: {}", styler.green("none")),
    }
}

fn state_file_path() -> PathBuf {
    env::var_os("BOB_VAULT_SYNC_STATE_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(|| state_home().join("bob-cli").join("vault-sync.json"))
}

fn state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-state"))
}

fn read_status_record(path: &Path) -> Result<Option<StatusRecord>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn write_status_record(path: &Path, status: &StatusRecord) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("json.tmp");
    let json =
        serde_json::to_string_pretty(status).map_err(io::Error::other)?;
    fs::write(&temp_path, json)?;
    fs::rename(temp_path, path)
}

fn refresh_status_shas(
    vault: &Path,
    child_env: &ChildEnv,
    status: &mut StatusRecord,
) {
    status.local_sha = rev_parse(vault, child_env, "HEAD").ok().flatten();
    status.remote_sha = ls_remote_master(vault, child_env).ok().flatten();
}

#[derive(Debug, Clone)]
struct CycleError {
    code: i32,
    message: String,
}

impl CycleError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }

    fn from_output(command: &str, output: Output) -> Self {
        let details = command_output_details(&output);
        let code = bob_env::exit_code(output.status);
        Self {
            code,
            message: format!("{command} failed{details}"),
        }
    }

    fn is_non_fast_forward(&self) -> bool {
        let lower = self.message.to_ascii_lowercase();
        lower.contains("non-fast-forward")
            || lower.contains("fetch first")
            || lower.contains("rejected")
    }
}

impl From<String> for CycleError {
    fn from(message: String) -> Self {
        Self::fatal(message)
    }
}

fn command_output_details(output: &Output) -> String {
    let mut details = String::new();
    if !output.stdout.is_empty() {
        details.push_str(": ");
        details.push_str(String::from_utf8_lossy(&output.stdout).trim());
    }
    if !output.stderr.is_empty() {
        if details.is_empty() {
            details.push_str(": ");
        } else {
            details.push('\n');
        }
        details.push_str(String::from_utf8_lossy(&output.stderr).trim());
    }
    details
}

fn print_log(options: &RunOptions, message: &str) {
    if !options.quiet {
        println!("[{}] {message}", log_timestamp());
    }
}

fn warn(styler: &Styler, message: &str) {
    eprintln!(
        "[{}] {}: {message}",
        log_timestamp(),
        styler.warning_prefix()
    );
}

fn now_rfc3339() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn log_timestamp() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn hostname_slug() -> String {
    env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            Command::new("hostname")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                })
        })
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
