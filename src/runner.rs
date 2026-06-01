use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{self, Command as ProcessCommand, ExitStatus, Stdio},
};

use clap::{builder::OsStringValueParser, Arg, Command as ClapCommand};

use crate::native::{self, NativeCommand};
use crate::scripts::{embedded_assets, script_by_command, EmbeddedAsset};

const SUBCOMMANDS: &[(&str, &str, &str, NativeCommand)] = &[
    (
        "pomodoro",
        "bob_pomodoro",
        "Show the current Bob Pomodoro status",
        NativeCommand::Pomodoro,
    ),
    (
        "pomodoro-runtimes",
        "bob_pomodoro_runtimes",
        "Annotate Bob Pomodoro ledger entries with runtimes",
        NativeCommand::PomodoroRuntimes,
    ),
    (
        "notify",
        "bob_notify",
        "Notify when the current Bob Pomodoro is complete",
        NativeCommand::Notify,
    ),
    (
        "sync",
        "bob_sync",
        "Sync the Bob Obsidian vault",
        NativeCommand::Sync,
    ),
    (
        "tmux-pomodoro",
        "tmux_bob_pomodoro",
        "Print Bob Pomodoro status for tmux",
        NativeCommand::TmuxPomodoro,
    ),
];

#[derive(Debug, Clone)]
pub struct RunnerError {
    message: String,
}

impl RunnerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RunnerError {}

pub fn run_bob() -> i32 {
    let matches = match build_cli().try_get_matches_from(env::args_os()) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                eprintln!(
                    "bob: failed to print command-line error: {print_error}"
                );
            }
            return exit_code;
        }
    };

    let Some((subcommand, sub_matches)) = matches.subcommand() else {
        return 2;
    };

    let Some((script_command, native_command)) =
        command_for_subcommand(subcommand)
    else {
        eprintln!("bob: unknown subcommand: {subcommand}");
        return 2;
    };

    let args = sub_matches
        .get_many::<OsString>("args")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();

    run_command_or_report("bob", script_command, native_command, args)
}

pub fn run_legacy(script_command: &'static str) -> i32 {
    let args = env::args_os().skip(1).collect();
    let Some(native_command) = native::command_for_script(script_command)
    else {
        return run_script_or_report(script_command, script_command, args);
    };

    run_command_or_report(script_command, script_command, native_command, args)
}

pub fn run_script(
    script_command: &str,
    args: Vec<OsString>,
) -> Result<i32, RunnerError> {
    let script = script_by_command(script_command).ok_or_else(|| {
        RunnerError::new(format!("unknown script command: {script_command}"))
    })?;
    let script_dir = materialize_scripts()?;
    let script_path = script_dir.join(script.install_path);
    let path = path_with_script_dir(&script_dir)?;

    let status = ProcessCommand::new(&script_path)
        .args(args)
        .env("PATH", path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            RunnerError::new(format!(
                "failed to run {} at {}: {error}",
                script.command,
                script_path.display()
            ))
        })?;

    Ok(exit_code(status))
}

pub fn materialize_scripts() -> Result<PathBuf, RunnerError> {
    let script_dir = script_cache_dir();
    fs::create_dir_all(&script_dir).map_err(|error| {
        RunnerError::new(format!(
            "failed to create script cache directory {}: {error}",
            script_dir.display()
        ))
    })?;

    for asset in embedded_assets() {
        write_asset(&script_dir, asset)?;
    }

    Ok(script_dir)
}

fn build_cli() -> ClapCommand {
    let mut command = ClapCommand::new("bob")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Bob command-line tools")
        .subcommand_required(true)
        .arg_required_else_help(true);

    for (subcommand, _, about, _) in SUBCOMMANDS {
        command = command.subcommand(delegate_subcommand(subcommand, about));
    }

    command
}

fn delegate_subcommand(name: &'static str, about: &'static str) -> ClapCommand {
    ClapCommand::new(name)
        .about(about)
        .disable_help_flag(true)
        .arg(
            Arg::new("args")
                .num_args(0..)
                .trailing_var_arg(true)
                .allow_hyphen_values(true)
                .value_parser(OsStringValueParser::new()),
        )
}

fn command_for_subcommand(
    subcommand: &str,
) -> Option<(&'static str, NativeCommand)> {
    SUBCOMMANDS
        .iter()
        .find_map(|(name, script_command, _, native_command)| {
            (*name == subcommand).then_some((*script_command, *native_command))
        })
}

fn run_command_or_report(
    invocation: &str,
    script_command: &'static str,
    native_command: NativeCommand,
    args: Vec<OsString>,
) -> i32 {
    if use_script_fallback() {
        return run_script_or_report(invocation, script_command, args);
    }

    native::run(native_command, args)
}

fn run_script_or_report(
    invocation: &str,
    script_command: &'static str,
    args: Vec<OsString>,
) -> i32 {
    match run_script(script_command, args) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("{invocation}: {error}");
            1
        }
    }
}

fn use_script_fallback() -> bool {
    matches!(
        env::var("BOB_CLI_USE_SCRIPT").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn script_cache_dir() -> PathBuf {
    cache_home().join("bob-cli").join("scripts").join(format!(
        "{}-{:016x}",
        env!("CARGO_PKG_VERSION"),
        embedded_assets_hash()
    ))
}

fn cache_home() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-cache"))
}

fn write_asset(
    script_dir: &Path,
    asset: EmbeddedAsset,
) -> Result<(), RunnerError> {
    let target = script_dir.join(asset.install_path);
    if let Ok(existing) = fs::read(&target)
        && existing == asset.contents
    {
        set_asset_permissions(&target, asset.executable).map_err(|error| {
            fs_error("set permissions on cached script asset", &target, error)
        })?;
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            fs_error(
                "create cached script asset parent directory",
                parent,
                error,
            )
        })?;
    }

    let temp_path = temporary_asset_path(&target)?;
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, asset.contents).map_err(|error| {
        fs_error("write cached script asset", &temp_path, error)
    })?;
    set_asset_permissions(&temp_path, asset.executable).map_err(|error| {
        fs_error("set permissions on cached script asset", &temp_path, error)
    })?;
    fs::rename(&temp_path, &target).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        fs_error("install cached script asset", &target, error)
    })?;
    set_asset_permissions(&target, asset.executable).map_err(|error| {
        fs_error("set permissions on cached script asset", &target, error)
    })?;

    Ok(())
}

fn temporary_asset_path(target: &Path) -> Result<PathBuf, RunnerError> {
    let file_name =
        target.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            RunnerError::new(format!(
                "cached script asset path has no file name: {}",
                target.display()
            ))
        })?;

    Ok(target.with_file_name(format!(".{file_name}.{}.tmp", process::id())))
}

fn path_with_script_dir(script_dir: &Path) -> Result<OsString, RunnerError> {
    let mut paths = vec![script_dir.to_path_buf()];
    if let Some(existing_path) =
        env::var_os("PATH").filter(|value| !value.is_empty())
    {
        paths.extend(env::split_paths(&existing_path));
    }

    env::join_paths(paths).map_err(|error| {
        RunnerError::new(format!(
            "failed to prepend {} to PATH: {error}",
            script_dir.display()
        ))
    })
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> RunnerError {
    RunnerError::new(format!("{action} {}: {error}", path.display()))
}

fn embedded_assets_hash() -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for asset in embedded_assets() {
        hash = fnv1a(hash, asset.install_path.as_bytes());
        hash = fnv1a(hash, &[0]);
        hash = fnv1a(hash, asset.source_path.as_bytes());
        hash = fnv1a(hash, &[0]);
        hash = fnv1a(hash, asset.contents);
        hash = fnv1a(hash, &[u8::from(asset.executable)]);
    }
    hash
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}

#[cfg(unix)]
fn set_asset_permissions(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_asset_permissions(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}
