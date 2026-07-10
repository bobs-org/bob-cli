use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{self, Command as ProcessCommand, ExitStatus, Stdio},
};

use clap::{
    builder::{
        styling::{AnsiColor, Styles},
        OsStringValueParser,
    },
    Arg, Command as ClapCommand,
};

use crate::native::{self, NativeCommand};
use crate::scripts::{embedded_assets, script_by_command, EmbeddedAsset};

#[derive(Debug, Clone, Copy)]
struct Subcommand {
    name: &'static str,
    script_command: Option<&'static str>,
    about: &'static str,
    native_command: NativeCommand,
}

// Keep this table sorted alphabetically by command name; the top-level help
// renders subcommands in declaration order, and `subcommands_are_sorted` guards
// the invariant.
const SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "bulk-git-commit",
        script_command: None,
        about: "Commit and push Bob vault Git changes",
        native_command: NativeCommand::BulkGitCommit,
    },
    Subcommand {
        name: "capture",
        script_command: None,
        about: "Capture a task or bullet into the Bob vault",
        native_command: NativeCommand::Capture,
    },
    Subcommand {
        name: "capture-sections",
        script_command: None,
        about: "List the non-Tasks sections of a capture note",
        native_command: NativeCommand::CaptureSections,
    },
    Subcommand {
        name: "capture-targets",
        script_command: None,
        about: "List capture routes for inbox, area, and active project notes",
        native_command: NativeCommand::CaptureTargets,
    },
    Subcommand {
        name: "highlights",
        script_command: None,
        about: "Sync Highlights PDF annotations into reference notes",
        native_command: NativeCommand::Highlights,
    },
    Subcommand {
        name: "move-done-tasks",
        script_command: None,
        about: "Move done and canceled tasks and maintain done links",
        native_command: NativeCommand::MoveDoneTasks,
    },
    Subcommand {
        name: "nightly",
        script_command: None,
        about: "Run the nightly Obsidian sync and maintenance steps",
        native_command: NativeCommand::Nightly,
    },
    Subcommand {
        name: "notify",
        script_command: Some("bob_notify"),
        about: "Notify when the current Pomodoro is complete",
        native_command: NativeCommand::Notify,
    },
    Subcommand {
        name: "plugins",
        script_command: None,
        about: "Manage Bob Obsidian plugins (list and sync to the vault)",
        native_command: NativeCommand::Plugins,
    },
    Subcommand {
        name: "pomodoro",
        script_command: Some("bob_pomodoro"),
        about: "Show the current Pomodoro status",
        native_command: NativeCommand::Pomodoro,
    },
    Subcommand {
        name: "projects",
        script_command: None,
        about: "Manage project notes via their ^prj tasks",
        native_command: NativeCommand::Projects,
    },
    Subcommand {
        name: "query",
        script_command: None,
        about: "Run Dataview queries against the Bob vault",
        native_command: NativeCommand::Query,
    },
    Subcommand {
        name: "tmux-pomodoro",
        script_command: Some("tmux_bob_pomodoro"),
        about: "Print Pomodoro status for tmux",
        native_command: NativeCommand::TmuxPomodoro,
    },
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

    run_command_or_report(
        script_command,
        Some(script_command),
        native_command,
        args,
    )
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

const ABOUT: &str =
    "Bob \u{2014} command-line tools for the Bob Obsidian vault and \
     Pomodoro workflow";

const LONG_ABOUT: &str =
    "Bob \u{2014} command-line tools for the Bob Obsidian \
vault and Pomodoro workflow.\n\n\
Bob tracks a daily Pomodoro ledger inside an Obsidian vault and keeps that \
vault synced through Git. Run a task with `bob <command>`; pass `--help` to a \
command for its own options.";

const HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}";

const AFTER_HELP: &str = "\
Examples:
  bob bulk-git-commit             Commit and push Bob vault Git changes
  bob capture buy milk @groceries
                                 Capture a task into groceries.md
  bob capture '@!dev:foobar' 'Some foobar task.'
                                 Capture and link a next Pomodoro task
  bob capture-sections --route cash --format json
                                 List picker sections for one capture target
  bob capture-targets --format json
                                 List picker targets for task capture
  bob query --source '#project'
                                 Print matching note paths
  bob highlights scan --dry-run
                                 Preview Highlights reference note sync
  bob move-done-tasks --threshold 10
                                 Move tasks and maintain done links
  bob nightly                    Run the nightly sync and maintenance steps
  bob plugins list               List Bob plugins and their vault sync state
  bob pomodoro                   Show today's Pomodoro status
  bob projects list              List project notes and ^prj task states

Run 'bob <command> --help' for more information on a command.";

fn cli_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Green.on_default().bold())
        .usage(AnsiColor::Green.on_default().bold())
        .literal(AnsiColor::Cyan.on_default().bold())
        .placeholder(AnsiColor::Cyan.on_default())
}

fn build_cli() -> ClapCommand {
    let mut command = ClapCommand::new("bob")
        .version(env!("CARGO_PKG_VERSION"))
        .about(ABOUT)
        .long_about(LONG_ABOUT)
        .styles(cli_styles())
        .help_template(HELP_TEMPLATE)
        .after_help(AFTER_HELP)
        .subcommand_required(true)
        .arg_required_else_help(true);

    for subcommand in SUBCOMMANDS {
        command = command
            .subcommand(delegate_subcommand(subcommand.name, subcommand.about));
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
) -> Option<(Option<&'static str>, NativeCommand)> {
    SUBCOMMANDS.iter().find_map(|command| {
        (command.name == subcommand)
            .then_some((command.script_command, command.native_command))
    })
}

fn run_command_or_report(
    invocation: &str,
    script_command: Option<&'static str>,
    native_command: NativeCommand,
    args: Vec<OsString>,
) -> i32 {
    if use_script_fallback()
        && let Some(script_command) = script_command
    {
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

#[cfg(test)]
mod tests {
    use super::SUBCOMMANDS;

    #[test]
    fn subcommands_are_sorted_alphabetically() {
        let names: Vec<&str> =
            SUBCOMMANDS.iter().map(|command| command.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "SUBCOMMANDS must stay sorted by command name so top-level help \
             renders alphabetically"
        );
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        // `debug_assert`s inside clap fire during help rendering; exercise the
        // full build so a malformed template or style is caught in tests.
        super::build_cli().debug_assert();
    }
}
