use crate::native::env as bob_env;
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{self, Command as ProcessCommand, ExitStatus, Stdio},
};

use clap::{
    builder::{
        styling::{AnsiColor, Style, Styles},
        OsStringValueParser, StyledStr,
    },
    error::ErrorKind,
    Arg, Command as ClapCommand,
};

use crate::native::{self, NativeCommand};
use crate::scripts::{embedded_assets, script_by_command, EmbeddedAsset};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Subcommand {
    pub(crate) name: &'static str,
    pub(crate) about: &'static str,
    pub(crate) section: Section,
    pub(crate) target: Target,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Target {
    Leaf(Leaf),
    Group(Group),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Leaf {
    pub(crate) native_command: NativeCommand,
    pub(crate) script_command: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Group {
    pub(crate) members: &'static [Member],
    pub(crate) default: Option<&'static str>,
    pub(crate) extra_about: Option<&'static str>,
    pub(crate) after_help: &'static str,
    pub(crate) usage: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    pub(crate) name: &'static str,
    pub(crate) about: &'static str,
    pub(crate) leaf: Leaf,
}

/// Workflow section shared by root help and root completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    DailyWorkflow,
    TasksAndProjects,
    Vault,
    Integrations,
    Setup,
    CaptureProtocol,
}

impl Section {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::DailyWorkflow => "Daily workflow",
            Self::TasksAndProjects => "Tasks and projects",
            Self::Vault => "Vault",
            Self::Integrations => "Integrations",
            Self::Setup => "Setup",
            Self::CaptureProtocol => "Capture protocol",
        }
    }

    pub(crate) const fn completion_group(self) -> &'static str {
        match self {
            Self::DailyWorkflow => "daily workflow",
            Self::TasksAndProjects => "tasks and projects",
            Self::Vault => "vault",
            Self::Integrations => "integrations",
            Self::Setup => "setup",
            Self::CaptureProtocol => "capture protocol",
        }
    }
}

pub(crate) const SECTIONS: &[Section] = &[
    Section::DailyWorkflow,
    Section::TasksAndProjects,
    Section::Vault,
    Section::Integrations,
    Section::Setup,
    Section::CaptureProtocol,
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Alias {
    pub(crate) from: &'static str,
    pub(crate) to: &'static [&'static str],
}

pub(crate) const ALIASES: &[Alias] = &[
    Alias {
        from: "highlights",
        to: &["ref"],
    },
    Alias {
        from: "highlights-ref",
        to: &["ref"],
    },
    Alias {
        from: "mark-next-tasks",
        to: &["task", "reconcile"],
    },
    Alias {
        from: "move-done-tasks",
        to: &["task", "archive"],
    },
    Alias {
        from: "notify",
        to: &["pomodoro", "notify"],
    },
    Alias {
        from: "randomize",
        to: &["task", "reroll"],
    },
    Alias {
        from: "task-status-hooks",
        to: &["task", "reconcile"],
    },
    Alias {
        from: "task-status-setter",
        to: &["task", "reconcile"],
    },
    Alias {
        from: "tmux-pomodoro",
        to: &["pomodoro", "tmux"],
    },
];

const TASK_MEMBERS: &[Member] = &[
    Member {
        name: "archive",
        about:
            "Move done and canceled tasks into done/ archives and repair links",
        leaf: Leaf {
            native_command: NativeCommand::MoveDoneTasks,
            script_command: None,
        },
    },
    Member {
        name: "reconcile",
        about:
            "Reconcile task statuses from the Pomodoro ledger and dependencies",
        leaf: Leaf {
            native_command: NativeCommand::TaskStatusHooks,
            script_command: None,
        },
    },
    Member {
        name: "reroll",
        about: "Re-roll due prioritized tasks within their priority windows",
        leaf: Leaf {
            native_command: NativeCommand::Randomize,
            script_command: None,
        },
    },
];

const POMODORO_MEMBERS: &[Member] = &[
    Member {
        name: "notify",
        about: "Notify when the current Pomodoro is complete",
        leaf: Leaf {
            native_command: NativeCommand::Notify,
            script_command: Some("bob_notify"),
        },
    },
    Member {
        name: "status",
        about: "Show the current Pomodoro status (default)",
        leaf: Leaf {
            native_command: NativeCommand::Pomodoro,
            script_command: Some("bob_pomodoro"),
        },
    },
    Member {
        name: "tmux",
        about: "Print the Pomodoro status and plan meter for tmux",
        leaf: Leaf {
            native_command: NativeCommand::TmuxPomodoro,
            script_command: Some("tmux_bob_pomodoro"),
        },
    },
];

const TASK_AFTER_HELP: &str = "\
Examples:
  bob task reconcile --dry-run   Preview task status reconciliation
  bob task reroll --dry-run      Preview re-rolling due prioritized tasks
  bob task archive               Archive done and canceled tasks

See also: bob plan, bob ready, bob freshness, bob projects, bob query
Run 'bob task <command> --help' for more information on a command.";

const POMODORO_AFTER_HELP: &str = "\
Bare `bob pomodoro [-d] [-s] [-v]` runs `bob pomodoro status`.

Examples:
  bob pomodoro                 Show the current Pomodoro status
  bob pomodoro -s              Include a stale open Pomodoro
  bob pomodoro tmux            Print the tmux status-line segment
  bob pomodoro notify 30 300   Check every 30 s; after notifying, wait 300 s

Run 'bob pomodoro <command> --help' for more information on a command.";

const GROUP_HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}";

/// Rewrite an exact root alias while retaining every remaining OsString.
/// Runtime dispatch and completion share this argv-prefix table.
pub(crate) fn rewrite_alias_args(args: &[OsString]) -> Vec<OsString> {
    let Some(first) = args.first() else {
        return args.to_vec();
    };
    let Some(alias) =
        ALIASES.iter().find(|alias| first == OsStr::new(alias.from))
    else {
        return args.to_vec();
    };

    alias
        .to
        .iter()
        .map(OsString::from)
        .chain(args.iter().skip(1).cloned())
        .collect()
}

pub(crate) fn subcommands() -> &'static [Subcommand] {
    SUBCOMMANDS
}

// Keep this table sorted by section and then command name. Root help and
// completion both render in declaration order.
const SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "capture",
        about: "Capture tasks, bullets, and Pomodoro commands into the vault",
        section: Section::DailyWorkflow,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Capture,
            script_command: None,
        }),
    },
    Subcommand {
        name: "freshness",
        about: "Walk the tiered freshness review queue",
        section: Section::DailyWorkflow,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Freshness,
            script_command: None,
        }),
    },
    Subcommand {
        name: "plan",
        about:
            "Show today's plan budget, Today's tasks, and NEXT/PENDING lanes",
        section: Section::DailyWorkflow,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Plan,
            script_command: None,
        }),
    },
    Subcommand {
        name: "pomodoro",
        about:
            "Show Pomodoro status, print the tmux line, or notify on completion",
        section: Section::DailyWorkflow,
        target: Target::Group(Group {
            members: POMODORO_MEMBERS,
            default: Some("status"),
            extra_about: None,
            after_help: POMODORO_AFTER_HELP,
            usage: "bob pomodoro [COMMAND]",
        }),
    },
    Subcommand {
        name: "ready",
        about:
            "Show each area/project note's Ready lane against the per-note cap",
        section: Section::DailyWorkflow,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::NoteReady,
            script_command: None,
        }),
    },
    Subcommand {
        name: "projects",
        about: "List and sync project notes via their ^prj tasks",
        section: Section::TasksAndProjects,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Projects,
            script_command: None,
        }),
    },
    Subcommand {
        name: "task",
        about: "Vault-wide task maintenance: reconcile, reroll, archive",
        section: Section::TasksAndProjects,
        target: Target::Group(Group {
            members: TASK_MEMBERS,
            default: None,
            extra_about: Some(
                "Every `bob task` command rewrites task lines across the whole vault.",
            ),
            after_help: TASK_AFTER_HELP,
            usage: "bob task <COMMAND>",
        }),
    },
    Subcommand {
        name: "nightly",
        about: "Run nightly maintenance: vault-sync, task archive, vault-sync",
        section: Section::Vault,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Nightly,
            script_command: None,
        }),
    },
    Subcommand {
        name: "query",
        about: "Run Dataview or Tasks queries against the vault",
        section: Section::Vault,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Query,
            script_command: None,
        }),
    },
    Subcommand {
        name: "ref",
        about: "Find, list, and read references; sync Highlights PDFs into them",
        section: Section::Vault,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Highlights,
            script_command: None,
        }),
    },
    Subcommand {
        name: "vault-sync",
        about: "Reconcile the vault through Git (default: run) or show status",
        section: Section::Vault,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::VaultSync,
            script_command: None,
        }),
    },
    Subcommand {
        name: "gkeep",
        about: "Drain the Google Keep inbox into Obsidian tasks",
        section: Section::Integrations,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Gkeep,
            script_command: None,
        }),
    },
    Subcommand {
        name: "completion",
        about: "Install and inspect shell completion for bob",
        section: Section::Setup,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Completion,
            script_command: None,
        }),
    },
    Subcommand {
        name: "plugins",
        about: "List and deploy Bob's custom Obsidian plugins",
        section: Section::Setup,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::Plugins,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-complete",
        about: "Complete the capture marker at the cursor",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureComplete,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-parse",
        about: "Explain what in-progress capture text currently means",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureParse,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-pomodoro-name",
        about: "Write a name onto an open unnamed Pomodoro",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CapturePomodoroName,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-pomodoros",
        about: "List today's Pomodoro ledger entries",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CapturePomodoros,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-rewrite",
        about: "Apply the capture grammar's automatic draft rewrites",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureRewrite,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-sections",
        about: "List the non-Tasks sections of a capture note",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureSections,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-targets",
        about: "List inbox, area, and active project capture routes",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureTargets,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-task-id",
        about: "Write a block ID onto an open capture task",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureTaskId,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-task-sections",
        about: "List the ALL-CAPS child sections of a capture task",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureTaskSections,
            script_command: None,
        }),
    },
    Subcommand {
        name: "capture-tasks",
        about: "List the open tasks of a capture note",
        section: Section::CaptureProtocol,
        target: Target::Leaf(Leaf {
            native_command: NativeCommand::CaptureTasks,
            script_command: None,
        }),
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
    let argv: Vec<OsString> = env::args_os().collect();
    // Hidden shell-completion endpoint. It intercepts before the root
    // clap parse and before the script fallback, and it never appears in
    // help output or the completion tree.
    if argv.get(1).is_some_and(|first| first == "__complete") {
        return crate::native::completion::run_complete(&argv[2..]);
    }

    let mut args = rewrite_alias_args(&argv[1..]);
    args = match route_help_tokens(args) {
        Ok(args) => args,
        Err(exit_code) => return exit_code,
    };
    args = insert_group_defaults(args);
    run_bob_with_args(
        std::iter::once(OsString::from("bob")).chain(args).collect(),
    )
}

fn route_help_tokens(args: Vec<OsString>) -> Result<Vec<OsString>, i32> {
    if args.first().is_some_and(|first| first == "help") {
        return route_help(&args[1..]);
    }
    if args.len() >= 2
        && args.get(1).is_some_and(|token| token == "help")
        && find_group(args[0].as_os_str()).is_some()
    {
        let mut path = vec![args[0].clone()];
        path.extend(args.iter().skip(2).cloned());
        return route_help(&path);
    }
    Ok(args)
}

fn insert_group_defaults(args: Vec<OsString>) -> Vec<OsString> {
    let Some(first) = args.first() else {
        return args;
    };
    let Some(group) = find_group(first.as_os_str()) else {
        return args;
    };
    let Some(default) = group.default else {
        return args;
    };

    let should_insert = match args.get(1) {
        None => true,
        Some(token) => {
            starts_with_dash(token.as_os_str())
                && !is_help_flag(token.as_os_str())
        }
    };
    if !should_insert {
        return args;
    }

    let mut out = Vec::with_capacity(args.len() + 1);
    out.push(first.clone());
    out.push(OsString::from(default));
    out.extend(args.iter().skip(1).cloned());
    out
}

fn starts_with_dash(token: &OsStr) -> bool {
    token.as_encoded_bytes().first() == Some(&b'-')
}

fn is_help_flag(token: &OsStr) -> bool {
    token == "-h" || token == "--help"
}

fn find_group(name: &OsStr) -> Option<&'static Group> {
    SUBCOMMANDS.iter().find_map(|entry| {
        if entry.name != name {
            return None;
        }
        match &entry.target {
            Target::Group(group) => Some(group),
            Target::Leaf(_) => None,
        }
    })
}

fn find_entry(name: &str) -> Option<&'static Subcommand> {
    SUBCOMMANDS.iter().find(|entry| entry.name == name)
}

fn route_help(path: &[OsString]) -> Result<Vec<OsString>, i32> {
    if path.is_empty() || (path.len() == 1 && path[0] == "--help") {
        return Ok(vec![OsString::from("--help")]);
    }

    // `bob help <path>` shares the alias table with dispatch. Aliases may
    // expand to a multi-token canonical path in a later command-tree phase.
    let path = rewrite_alias_args(path);
    if let Some(invalid) = invalid_help_path(&path) {
        let mut command = build_cli();
        let error = command.error(
            ErrorKind::InvalidSubcommand,
            format!("unrecognized subcommand '{invalid}'"),
        );
        let exit_code = error.exit_code();
        if let Err(print_error) = error.print() {
            eprintln!("bob: failed to print command-line error: {print_error}");
        }
        return Err(exit_code);
    }

    Ok(path
        .into_iter()
        .chain(std::iter::once(OsString::from("--help")))
        .collect())
}

fn invalid_help_path(path: &[OsString]) -> Option<String> {
    let root = crate::native::completion::tree();
    let mut command = &root;
    for token in path {
        let Some(name) = token.to_str() else {
            return Some(token.to_string_lossy().into_owned());
        };
        let Some(next) = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == name)
        else {
            return Some(name.to_owned());
        };
        command = next;
    }
    None
}

fn run_bob_with_args(argv: Vec<OsString>) -> i32 {
    let matches = match build_cli().try_get_matches_from(argv) {
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

    let Some(leaf) = leaf_for_matches(subcommand, sub_matches) else {
        eprintln!("bob: unknown subcommand: {subcommand}");
        return 2;
    };

    let args = trailing_args(leaf_matches(subcommand, sub_matches));

    run_command_or_report("bob", leaf.script_command, leaf.native_command, args)
}

fn leaf_for_matches(
    subcommand: &str,
    sub_matches: &clap::ArgMatches,
) -> Option<Leaf> {
    let entry = find_entry(subcommand)?;
    match entry.target {
        Target::Leaf(leaf) => Some(leaf),
        Target::Group(group) => {
            let (member_name, _) = sub_matches.subcommand()?;
            group
                .members
                .iter()
                .find(|member| member.name == member_name)
                .map(|member| member.leaf)
        }
    }
}

fn leaf_matches<'a>(
    subcommand: &str,
    sub_matches: &'a clap::ArgMatches,
) -> &'a clap::ArgMatches {
    match find_entry(subcommand).map(|entry| entry.target) {
        Some(Target::Group(_)) => sub_matches
            .subcommand()
            .map(|(_, matches)| matches)
            .unwrap_or(sub_matches),
        _ => sub_matches,
    }
}

fn trailing_args(matches: &clap::ArgMatches) -> Vec<OsString> {
    matches
        .get_many::<OsString>("args")
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
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
    "Bob tracks a daily Pomodoro ledger inside an Obsidian vault and keeps that\n\
vault synced through Git. Commands follow the daily workflow: capture, review\n\
(plan, freshness, ready), run Pomodoros, reconcile tasks, and nightly\n\
maintenance. Pass `--help` to any command, or run `bob help <command>`, for\n\
its own options.";

const HELP_TEMPLATE: &str = "\
{about-with-newline}
{usage-heading} {usage}

{before-help}{all-args}{after-help}";

const AFTER_HELP: &str = "\
Examples:
  bob capture buy milk @groceries   Capture a task into groceries.md
  bob capture '='                   Start the queued Pomodoro
  bob plan                          Show today's plan budget and lanes
  bob freshness                     List the review queue
  bob ready                         Show Ready lanes against the per-note cap
  bob task reconcile --dry-run      Preview task status reconciliation
  bob query --source '#project'     Print matching note paths
  bob vault-sync status --json      Print the last vault Git sync status

Run 'bob <command> --help' or 'bob help <command>' for more on a command.";

fn sectioned_help(long: bool) -> StyledStr {
    let mut help = StyledStr::new();
    let color =
        io::stdout().is_terminal() && bob_env::var_os("NO_COLOR").is_none();
    let styles = cli_styles();
    let name_width = SUBCOMMANDS
        .iter()
        .filter(|entry| entry.section != Section::CaptureProtocol)
        .map(|entry| entry.name.chars().count())
        .max()
        .unwrap_or(0);
    let protocol_width = SUBCOMMANDS
        .iter()
        .filter(|entry| entry.section == Section::CaptureProtocol)
        .map(|entry| entry.name.chars().count())
        .max()
        .unwrap_or(0);

    let mut first_section = true;
    for section in SECTIONS {
        let entries: Vec<&Subcommand> = SUBCOMMANDS
            .iter()
            .filter(|entry| entry.section == *section)
            .collect();
        if entries.is_empty() {
            continue;
        }
        if !first_section {
            help.push_str("\n");
        }
        first_section = false;
        let title = if *section == Section::CaptureProtocol && !long {
            "Capture protocol (Bob Mac Capture JSON endpoints; `bob --help` lists them)"
        } else {
            section.title()
        };
        push_help_styled(&mut help, title, styles.get_header(), color);
        help.push_str(":\n");

        if *section == Section::CaptureProtocol && !long {
            append_wrapped_names(&mut help, &entries);
        } else {
            let width = if *section == Section::CaptureProtocol {
                protocol_width
            } else {
                name_width
            };
            for entry in entries {
                append_help_row(
                    &mut help,
                    entry,
                    width,
                    long || *section != Section::CaptureProtocol,
                    styles.get_literal(),
                    color,
                );
            }
        }
    }
    StyledStr::from(format!("{help}").trim_end().to_owned())
}

fn push_help_styled(
    output: &mut StyledStr,
    text: &str,
    style: &Style,
    color: bool,
) {
    if color {
        output.push_str(&format!(
            "{}{}{}",
            style.render(),
            text,
            style.render_reset()
        ));
    } else {
        output.push_str(text);
    }
}

fn append_help_row(
    output: &mut StyledStr,
    entry: &Subcommand,
    name_width: usize,
    show_about: bool,
    literal_style: &Style,
    color: bool,
) {
    let prefix = format!("  {:<name_width$}  ", entry.name);
    let desc = if show_about { entry.about } else { "" };
    let available = 80usize.saturating_sub(prefix.chars().count());
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in desc.split_whitespace() {
        let next_width = line.chars().count()
            + usize::from(!line.is_empty())
            + word.chars().count();
        if next_width > available && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    output.push_str("  ");
    push_help_styled(output, entry.name, literal_style, color);
    output.push_str(&format!(
        "{}{}\n",
        " ".repeat(name_width - entry.name.len() + 2),
        lines[0]
    ));
    let continuation = " ".repeat(prefix.chars().count());
    for line in lines.iter().skip(1) {
        output.push_str(&continuation);
        output.push_str(line);
        output.push_str("\n");
    }
}

fn append_wrapped_names(output: &mut StyledStr, entries: &[&Subcommand]) {
    let mut line = String::from("  ");
    for entry in entries {
        let addition = if line.len() == 2 {
            entry.name.to_owned()
        } else {
            format!("  {}", entry.name)
        };
        if line.chars().count() + addition.chars().count() > 80
            && line.len() > 2
        {
            output.push_str(&line);
            output.push_str("\n");
            line.clear();
            line.push_str("  ");
            line.push_str(entry.name);
        } else {
            line.push_str(&addition);
        }
    }
    if line.len() > 2 {
        output.push_str(&line);
        output.push_str("\n");
    }
}

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
        .override_usage("bob <COMMAND>")
        .after_help(AFTER_HELP)
        .before_help(sectioned_help(false))
        .before_long_help(sectioned_help(true))
        .disable_help_subcommand(true)
        .subcommand_required(true)
        .arg_required_else_help(true);

    for subcommand in SUBCOMMANDS {
        command =
            command.subcommand(dispatch_subcommand(subcommand).hide(true));
    }

    command
}

fn dispatch_subcommand(entry: &Subcommand) -> ClapCommand {
    match entry.target {
        Target::Leaf(_) => delegate_subcommand(entry.name, entry.about),
        Target::Group(group) => group_dispatch_command(entry, group),
    }
}

fn group_dispatch_command(entry: &Subcommand, group: Group) -> ClapCommand {
    let about = match group.extra_about {
        Some(extra) => format!("{}\n\n{extra}", entry.about),
        None => entry.about.to_string(),
    };
    let mut command = ClapCommand::new(entry.name)
        .about(about)
        .styles(cli_styles())
        .help_template(GROUP_HELP_TEMPLATE)
        .override_usage(group.usage)
        .after_help(group.after_help)
        .disable_help_subcommand(true);
    if group.default.is_none() {
        command = command
            .subcommand_required(true)
            .arg_required_else_help(true);
    }
    for member in group.members {
        command =
            command.subcommand(delegate_subcommand(member.name, member.about));
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

#[cfg(test)]
pub(crate) fn canonical_leaves() -> Vec<(Vec<&'static str>, Leaf)> {
    let mut leaves = Vec::new();
    for entry in SUBCOMMANDS {
        match entry.target {
            Target::Leaf(leaf) => leaves.push((vec![entry.name], leaf)),
            Target::Group(group) => {
                for member in group.members {
                    leaves.push((vec![entry.name, member.name], member.leaf));
                }
            }
        }
    }
    leaves
}

#[cfg(test)]
pub(crate) fn resolve_path(path: &[&str]) -> Option<Leaf> {
    let (first, rest) = path.split_first()?;
    let entry = find_entry(first)?;
    match entry.target {
        Target::Leaf(leaf) if rest.is_empty() => Some(leaf),
        Target::Group(group) if rest.len() == 1 => group
            .members
            .iter()
            .find(|member| member.name == rest[0])
            .map(|member| member.leaf),
        _ => None,
    }
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
        bob_env::var("BOB_CLI_USE_SCRIPT").ok().as_deref(),
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
    bob_env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            bob_env::var_os("HOME")
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
        bob_env::var_os("PATH").filter(|value| !value.is_empty())
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
    use std::ffi::OsString;

    use super::{rewrite_alias_args, Section, SECTIONS, SUBCOMMANDS};

    #[test]
    fn subcommands_are_contiguous_in_section_order_and_alphabetical() {
        let mut last_section = None;
        let mut last_name = "";
        let mut seen_sections = Vec::new();
        for command in SUBCOMMANDS {
            if last_section != Some(command.section) {
                if let Some(previous) = last_section {
                    assert!(
                        !seen_sections.contains(&command.section),
                        "section {:?} is not contiguous",
                        command.section
                    );
                    let previous_index = SECTIONS
                        .iter()
                        .position(|section| *section == previous)
                        .unwrap();
                    let current_index = SECTIONS
                        .iter()
                        .position(|section| *section == command.section)
                        .unwrap();
                    assert!(current_index > previous_index);
                }
                seen_sections.push(command.section);
                last_section = Some(command.section);
                last_name = "";
            }
            assert!(
                command.name > last_name,
                "{} is out of alphabetical order in {:?}",
                command.name,
                command.section
            );
            last_name = command.name;
        }
        assert_eq!(
            seen_sections,
            SECTIONS
                .iter()
                .copied()
                .filter(|section| SUBCOMMANDS
                    .iter()
                    .any(|entry| entry.section == *section))
                .collect::<Vec<Section>>()
        );
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        // `debug_assert`s inside clap fire during help rendering; exercise the
        // full build so a malformed template or style is caught in tests.
        super::build_cli().debug_assert();
    }

    #[cfg(unix)]
    #[test]
    fn alias_rewrite_preserves_separator_and_non_utf8_tail() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let args = vec![
            OsString::from("task-status-setter"),
            OsString::from("--"),
            OsString::from_vec(vec![0xff, b'x']),
        ];
        let rewritten = rewrite_alias_args(&args);
        assert_eq!(rewritten[0], "task");
        assert_eq!(rewritten[1], "reconcile");
        assert_eq!(rewritten[2], "--");
        assert_eq!(rewritten[3].as_os_str().as_bytes(), [0xff, b'x']);
    }

    #[test]
    fn aliases_resolve_to_leaves_and_do_not_collide_with_root_names() {
        let roots: Vec<&str> =
            SUBCOMMANDS.iter().map(|entry| entry.name).collect();
        for alias in super::ALIASES {
            assert!(
                !roots.contains(&alias.from),
                "alias {} collides with a root command",
                alias.from
            );
            assert!(
                super::resolve_path(alias.to).is_some(),
                "alias {} target {:?} is not a leaf",
                alias.from,
                alias.to
            );
        }
    }

    #[test]
    fn every_native_command_is_on_exactly_one_canonical_path() {
        use crate::native::NativeCommand;

        let leaves = super::canonical_leaves();
        let mut commands: Vec<NativeCommand> =
            leaves.iter().map(|(_, leaf)| leaf.native_command).collect();
        let total = commands.len();
        commands.sort_by_key(|command| format!("{command:?}"));
        commands.dedup();
        assert_eq!(
            commands.len(),
            total,
            "a NativeCommand is mounted on more than one canonical path"
        );

        let expected = [
            NativeCommand::Capture,
            NativeCommand::CaptureComplete,
            NativeCommand::CaptureParse,
            NativeCommand::CapturePomodoroName,
            NativeCommand::CapturePomodoros,
            NativeCommand::CaptureRewrite,
            NativeCommand::CaptureSections,
            NativeCommand::CaptureTargets,
            NativeCommand::CaptureTaskId,
            NativeCommand::CaptureTaskSections,
            NativeCommand::CaptureTasks,
            NativeCommand::Completion,
            NativeCommand::Freshness,
            NativeCommand::Gkeep,
            NativeCommand::Query,
            NativeCommand::Highlights,
            NativeCommand::MoveDoneTasks,
            NativeCommand::Nightly,
            NativeCommand::NoteReady,
            NativeCommand::Notify,
            NativeCommand::Plan,
            NativeCommand::Plugins,
            NativeCommand::Pomodoro,
            NativeCommand::Projects,
            NativeCommand::Randomize,
            NativeCommand::TaskStatusHooks,
            NativeCommand::TmuxPomodoro,
            NativeCommand::VaultSync,
        ];
        assert_eq!(commands.len(), expected.len());
        for command in expected {
            assert!(
                leaves
                    .iter()
                    .any(|(_, leaf)| leaf.native_command == command),
                "NativeCommand::{command:?} is not on a canonical path"
            );
        }
    }
}
