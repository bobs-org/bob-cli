//! The public `bob completion` command tree.
//!
//! Completion's own arguments use clap possible values and
//! [`clap::ValueHint`]s, so they never need `kinds.rs` entries.

use clap::{
    builder::{PossibleValue, PossibleValuesParser},
    Arg, ArgAction, Command as ClapCommand,
};

const COMMAND_NAME: &str = "bob completion";

/// A shell `bob completion` can install an adapter for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Shell {
    Bash,
    Zsh,
}

impl Shell {
    /// Every shell this binary supports, in report order.
    pub(crate) fn all() -> &'static [Shell] {
        &[Shell::Bash, Shell::Zsh]
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
        }
    }

    /// The adapter file name inside the target directory.
    pub(crate) fn file_name(self) -> &'static str {
        match self {
            Shell::Bash => "bob",
            Shell::Zsh => "_bob",
        }
    }

    /// The function name the shell should bind `bob` to.
    pub(crate) fn function_name(self) -> &'static str {
        match self {
            Shell::Bash => "_bob",
            Shell::Zsh => "_bob",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Shell> {
        match raw {
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            _ => None,
        }
    }

    /// Possible values for the `SHELL` positional, with help text.
    pub(crate) fn possible_values() -> Vec<PossibleValue> {
        vec![
            PossibleValue::new("bash")
                .help("The Bourne-again shell (values only)"),
            PossibleValue::new("zsh").help("The Z shell"),
        ]
    }
}

impl std::fmt::Display for Shell {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Install and inspect shell completion for bob")
        .long_about(
            "Install and inspect shell completion for bob.\n\n\
             Every <TAB> is answered live by the `bob` on your PATH, so \
             completion always matches the installed binary: commands, \
             options, and vault values such as capture routes, tasks, and \
             open Pomodoros. The file installed into your shell is a small, \
             stable adapter that rarely changes. `bob completion install` \
             keeps it current and never edits your rc files.",
        )
        .after_help(
            "Examples:\n  bob completion                   Same as `bob completion status`\n  bob completion install           Install for $SHELL; refresh every bob-owned adapter\n  bob completion install zsh -d    Show the zsh plan without writing anything\n  bob completion status -v         Also check registration in a real shell\n  bob completion zsh -o ~/.zfunc/_bob\n                                   Write the zsh adapter yourself",
        )
        .arg(json_arg())
        .arg(verify_arg())
        .args_conflicts_with_subcommands(true)
        .subcommand(install_command())
        .subcommand(status_command())
        .subcommand(uninstall_command())
        .subcommand(bash_command())
        .subcommand(zsh_command())
}

/// The completion-only builder mounted in the shared [`tree`](super::tree).
///
/// Identical to the runtime builder: the bare `bob completion [-j] [-v]`
/// status form is modeled by the top-level flags plus
/// `args_conflicts_with_subcommands`.
pub(crate) fn completion_command() -> ClapCommand {
    build_cli()
}

fn shell_positional(help: &'static str) -> Arg {
    Arg::new("shell")
        .help(help)
        .num_args(0..)
        .value_parser(PossibleValuesParser::new(Shell::possible_values()))
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .short('d')
        .long("dry-run")
        .help("Show the plan; write nothing, not even the manifest")
        .action(ArgAction::SetTrue)
}

fn json_arg() -> Arg {
    Arg::new("json")
        .short('j')
        .long("json")
        .help("Print machine-readable JSON")
        .action(ArgAction::SetTrue)
}

fn verify_arg() -> Arg {
    Arg::new("verify")
        .short('v')
        .long("verify")
        .help("Probe a real shell now instead of trusting the install record")
        .action(ArgAction::SetTrue)
}

fn install_command() -> ClapCommand {
    ClapCommand::new("install")
        .about("Install or refresh the completion adapter for your shells")
        .long_about(
            "Install or refresh the completion adapter for your shells.\n\n\
             With no SHELL, bob installs for $SHELL and refreshes every \
             bob-owned adapter. For zsh the target is the first match: \
             --target, the previous install location, the first writable \
             fpath entry under $HOME, oh-my-zsh completions, then ~/.zfunc. \
             For bash it is ${BASH_COMPLETION_USER_DIR:-${XDG_DATA_HOME:-~/.local/share}/bash-completion}/completions/bob. \
             Files bob did not write are refused without --force, and your \
             rc files are never edited.",
        )
        .after_help(
            "Examples:\n  bob completion install\n  bob completion install zsh\n  bob completion install zsh -d\n  bob completion install zsh -t ~/.zfunc",
        )
        .arg(shell_positional(
            "Which shells to install (default: $SHELL plus owned adapters)",
        ))
        .arg(dry_run_arg())
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .help("Replace a file bob did not write, or one edited since install")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-verify")
                .short('n')
                .long("no-verify")
                .help("Skip the real-shell registration check")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .help("Print only warnings and errors")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("target")
                .short('t')
                .long("target")
                .help("Install into DIR (only with a single shell)")
                .value_name("DIR")
                .value_hint(clap::ValueHint::DirPath),
        )
}

fn status_command() -> ClapCommand {
    ClapCommand::new("status")
        .about("Show installed adapters and the bob they call")
        .long_about(
            "Show installed adapters and the bob they call.\n\n\
             Without --verify, registration is the result recorded at \
             install time, shown only when it still matches the file. \
             With --verify, bob probes a real shell now.",
        )
        .after_help(
            "Examples:\n  bob completion\n  bob completion status\n  bob completion status -v\n  bob completion status -j",
        )
        .alias("list")
        .arg(json_arg())
        .arg(verify_arg())
}

fn uninstall_command() -> ClapCommand {
    ClapCommand::new("uninstall")
        .about("Remove completion adapters that bob installed")
        .long_about(
            "Remove completion adapters that bob installed.\n\n\
             With no SHELL, every bob-owned adapter is removed. Only files \
             whose stamp and manifest digest prove bob wrote them are \
             removed; an edited file is refused with the exact `rm` command \
             instead.",
        )
        .after_help(
            "Examples:\n  bob completion uninstall\n  bob completion uninstall zsh\n  bob completion uninstall zsh -d",
        )
        .arg(shell_positional("Which shells to uninstall (default: owned adapters)"))
        .arg(dry_run_arg())
}

fn bash_command() -> ClapCommand {
    ClapCommand::new("bash")
        .about("Print the bash completion adapter")
        .long_about(
            "Print the bash completion adapter.\n\n\
             Writes the adapter yourself instead of running install; no \
             manifest entry is recorded, so status reports the file as \
             externally managed when its bytes match.",
        )
        .after_help(
            "Examples:\n  bob completion bash\n  bob completion bash -o ~/.local/share/bash-completion/completions/bob",
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .help("Write the adapter to FILE instead of stdout")
                .value_name("FILE")
                .value_hint(clap::ValueHint::FilePath),
        )
}

fn zsh_command() -> ClapCommand {
    ClapCommand::new("zsh")
        .about("Print the zsh completion adapter")
        .long_about(
            "Print the zsh completion adapter.\n\n\
             Writes the adapter yourself instead of running install; no \
             manifest entry is recorded, so status reports the file as \
             externally managed when its bytes match.",
        )
        .after_help(
            "Examples:\n  bob completion zsh\n  bob completion zsh -o ~/.zfunc/_bob",
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .help("Write the adapter to FILE instead of stdout")
                .value_name("FILE")
                .value_hint(clap::ValueHint::FilePath),
        )
}
