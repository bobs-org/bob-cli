use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use chrono::{Datelike, Local};
use fs2::FileExt;

use super::env as bob_env;

const SYNC_ALREADY_RUNNING_MESSAGE: &str =
    "Another sync instance is already running for this vault.";

struct Context {
    ob_command: OsString,
    child_env: Vec<(OsString, OsString)>,
}

pub(crate) fn run(_args: Vec<OsString>) -> i32 {
    let bob_dir = bob_env::bob_dir();
    let commit_message = env::var("BOB_SYNC_COMMIT_MESSAGE")
        .unwrap_or_else(|_| format!("bob sync {}", today()));

    let _lock = match acquire_lock() {
        Ok(lock) => lock,
        Err(code) => return code,
    };

    let ob_command = match load_ob_command() {
        Ok(command) => command,
        Err(message) => return die(&message),
    };

    let child_env = child_env();
    let context = Context {
        ob_command,
        child_env,
    };

    if let Err(message) = verify_bob_worktree(&context, &bob_dir) {
        return die(&message);
    }

    log(format_args!(
        "Running Obsidian sync for {}.",
        bob_dir.display()
    ));
    if let Err(code) = run_ob_sync(&context, &bob_dir) {
        return code;
    }

    log(format_args!(
        "Obsidian sync status for {}:",
        bob_dir.display()
    ));
    if let Err(code) =
        run_ob_command(&context, ["sync-status", "--path"], &bob_dir)
    {
        return code;
    }

    log(format_args!("Staging Bob vault changes."));
    if let Err(code) = run_git_command(&context, &bob_dir, ["add", "-A", "."]) {
        return code;
    }

    match git_diff_cached(&context, &bob_dir) {
        Ok(false) => {
            log(format_args!("No staged Bob changes to commit."));
        }
        Ok(true) => {
            log(format_args!("Committing Bob vault changes."));
            if let Err(code) =
                run_git_commit(&context, &bob_dir, &commit_message)
            {
                return code;
            }
        }
        Err(code) => return code,
    }

    log(format_args!("Pushing Bob vault commits."));
    if let Err(code) = run_git_command(&context, &bob_dir, ["push"]) {
        return code;
    }

    0
}

fn acquire_lock() -> Result<Option<File>, i32> {
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
            return Err(die(&format!(
                "Could not open lock file {}: {error}",
                lock_file.display()
            )));
        }
    };

    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            log(format_args!(
                "Another bob_sync run is already active; exiting."
            ));
            Err(0)
        }
        Err(error) => Err(die(&format!(
            "Could not acquire lock file {}: {error}",
            lock_file.display()
        ))),
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

fn load_ob_command() -> Result<OsString, String> {
    if let Some(path) = find_command_in_path("ob") {
        return Ok(path.into_os_string());
    }

    if let Some(path) = load_ob_from_nvm() {
        return Ok(path.into_os_string());
    }

    Err("Could not find the ob command after loading NVM.".to_string())
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

fn verify_bob_worktree(
    context: &Context,
    bob_dir: &Path,
) -> Result<(), String> {
    if !bob_dir.is_dir() {
        return Err(format!(
            "Bob directory does not exist: {}",
            bob_dir.display()
        ));
    }

    let status = git_command(context, bob_dir)
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

fn run_ob_sync(context: &Context, bob_dir: &Path) -> Result<(), i32> {
    let output = ob_command(context)
        .arg("sync")
        .arg("--path")
        .arg(bob_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| die(&format!("failed to run ob sync: {error}")))?;

    let sync_output = merged_output(&output);
    if output.status.success() {
        write_stdout_output(&sync_output);
        return Ok(());
    }

    if sync_output.contains(SYNC_ALREADY_RUNNING_MESSAGE) {
        log(format_args!(
            "Obsidian sync is already running for this vault; continuing."
        ));
        return Ok(());
    }

    write_stderr_output(&sync_output);
    Err(bob_env::exit_code(output.status))
}

fn run_ob_command<const N: usize>(
    context: &Context,
    args: [&str; N],
    path: &Path,
) -> Result<(), i32> {
    let status = ob_command(context)
        .args(args)
        .arg(path)
        .status()
        .map_err(|error| die(&format!("failed to run ob: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| bob_env::exit_code(status))
}

fn run_git_command<const N: usize>(
    context: &Context,
    bob_dir: &Path,
    args: [&str; N],
) -> Result<(), i32> {
    let status = git_command(context, bob_dir)
        .args(args)
        .status()
        .map_err(|error| die(&format!("failed to run git: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| bob_env::exit_code(status))
}

fn run_git_commit(
    context: &Context,
    bob_dir: &Path,
    message: &str,
) -> Result<(), i32> {
    let status = git_command(context, bob_dir)
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

fn git_diff_cached(context: &Context, bob_dir: &Path) -> Result<bool, i32> {
    let status = git_command(context, bob_dir)
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

fn ob_command(context: &Context) -> Command {
    let mut command = Command::new(&context.ob_command);
    command.envs(context.child_env.iter().cloned());
    command
}

fn git_command(context: &Context, bob_dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(bob_dir)
        .envs(context.child_env.iter().cloned());
    command
}

fn child_env() -> Vec<(OsString, OsString)> {
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

fn source_ssh_agent_env() -> Vec<(OsString, OsString)> {
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

fn find_command_in_path(command: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        candidate.is_file().then_some(candidate)
    })
}

fn today() -> String {
    let now = Local::now();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
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
