//! One composed clap command tree for completion.
//!
//! The tree is exhaustive over `runner::subcommands()` plus descriptors
//! for the five hand-parsed commands and the bare default-subcommand
//! forms. It is used only for completion; runtime dispatch stays
//! untouched.

use clap::Command as ClapCommand;

use crate::native::NativeCommand;
use crate::runner::{subcommands, Group, Subcommand, Target};

const ABOUT: &str = "Bob \u{2014} command-line tools for the Bob Obsidian vault and Pomodoro workflow";

impl NativeCommand {
    /// The completion-only clap builder for this command.
    ///
    /// Exhaustive over every variant with no wildcard arm, so a new
    /// `NativeCommand` cannot compile without a tree entry.
    pub(crate) fn command(self) -> ClapCommand {
        match self {
            NativeCommand::Capture => crate::native::capture::cli::build_cli(),
            NativeCommand::CaptureComplete => {
                crate::native::capture_complete::build_cli()
            }
            NativeCommand::CaptureParse => {
                crate::native::capture_parse::build_cli()
            }
            NativeCommand::CapturePomodoroName => {
                crate::native::capture_pomodoro_name::build_cli()
            }
            NativeCommand::CapturePomodoros => {
                crate::native::capture_pomodoros::build_cli()
            }
            NativeCommand::CaptureRewrite => {
                crate::native::capture_rewrite::build_cli()
            }
            NativeCommand::CaptureSections => {
                crate::native::capture_sections::build_cli()
            }
            NativeCommand::CaptureTargets => {
                crate::native::capture_targets::build_cli()
            }
            NativeCommand::CaptureTaskId => {
                crate::native::capture_task_id::build_cli()
            }
            NativeCommand::CaptureTaskSections => {
                crate::native::capture_task_sections::build_cli()
            }
            NativeCommand::CaptureTasks => {
                crate::native::capture_tasks::build_cli()
            }
            NativeCommand::Completion => {
                crate::native::completion::cli::completion_command()
            }
            NativeCommand::Freshness => {
                crate::native::freshness::cli::completion_command()
            }
            NativeCommand::Gkeep => crate::native::gkeep::cli::build_cli(),
            NativeCommand::Query => crate::native::dataview::cli::build_cli(),
            NativeCommand::Highlights => {
                crate::native::highlights_ref::cli::build_cli()
            }
            NativeCommand::MoveDoneTasks => {
                crate::native::collect_done::completion_descriptor()
            }
            NativeCommand::Nightly => {
                crate::native::nightly::completion_descriptor()
            }
            NativeCommand::NoteReady => {
                crate::native::note_ready::cli::build_cli()
            }
            NativeCommand::Notify => {
                crate::native::notify::completion_descriptor()
            }
            NativeCommand::Plan => crate::native::plan_budget::cli::build_cli(),
            NativeCommand::Plugins => crate::native::plugins::build_cli(),
            NativeCommand::Pomodoro => {
                crate::native::pomodoro::completion_descriptor()
            }
            NativeCommand::Projects => crate::native::projects::build_cli(),
            NativeCommand::Randomize => crate::native::randomize::build_cli(),
            NativeCommand::TaskStatusHooks => {
                crate::native::task_status_hooks::build_cli()
            }
            NativeCommand::TmuxPomodoro => {
                crate::native::pomodoro::tmux_completion_descriptor()
            }
            NativeCommand::VaultSync => {
                crate::native::vault_sync::completion_command()
            }
        }
    }
}

/// The full bob grammar used only for completion.
pub(crate) fn tree() -> ClapCommand {
    let mut root = ClapCommand::new("bob")
        .version(env!("CARGO_PKG_VERSION"))
        .about(ABOUT)
        .disable_help_subcommand(true);
    for entry in subcommands() {
        root = root.subcommand(completion_subcommand(entry));
    }
    root
}

fn completion_subcommand(entry: &Subcommand) -> ClapCommand {
    match entry.target {
        Target::Leaf(leaf) => leaf
            .native_command
            .command()
            .name(entry.name)
            .about(entry.about),
        Target::Group(group) => group_completion_command(entry, group),
    }
}

fn group_completion_command(entry: &Subcommand, group: Group) -> ClapCommand {
    let mut command = ClapCommand::new(entry.name)
        .about(entry.about)
        .disable_help_subcommand(true);
    if let Some(default) = group.default {
        let default_member = group
            .members
            .iter()
            .find(|member| member.name == default)
            .expect("group default must name a member");
        let default_command = default_member.leaf.native_command.command();
        command = command
            .disable_help_flag(true)
            .args_conflicts_with_subcommands(true);
        for arg in default_command.get_arguments() {
            let id = arg.get_id().as_str();
            if id == "help" || id == "version" {
                continue;
            }
            command = command.arg(arg.clone());
        }
    }
    for member in group.members {
        command = command.subcommand(
            member
                .leaf
                .native_command
                .command()
                .name(member.name)
                .about(member.about),
        );
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounted_names_match_subcommands_in_order() {
        let expected: Vec<&str> =
            subcommands().iter().map(|entry| entry.name).collect();
        let mounted: Vec<String> = tree()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .collect();
        assert_eq!(mounted, expected);
    }

    #[test]
    fn mounted_names_have_no_spaces() {
        for command in tree().get_subcommands() {
            assert!(
                !command.get_name().contains(' '),
                "mounted name contains a space: {}",
                command.get_name()
            );
        }
    }

    #[test]
    fn hidden_aliases_and_help_are_absent() {
        let root = tree();
        let names: Vec<&str> = root
            .get_subcommands()
            .map(|command| command.get_name())
            .collect();
        for hidden in crate::runner::ALIASES
            .iter()
            .map(|alias| alias.from)
            .chain(std::iter::once("help"))
        {
            assert!(!names.contains(&hidden), "tree must not contain {hidden}");
        }
        // No nested `help` subcommand either: disabling the help
        // subcommand is a global setting, so one probe per top-level
        // command is enough to catch a regression.
        for command in root.get_subcommands() {
            let nested: Vec<&str> = command
                .get_subcommands()
                .map(|sub| sub.get_name())
                .collect();
            assert!(
                !nested.contains(&"help"),
                "nested help subcommand under {}",
                command.get_name()
            );
        }
    }

    #[test]
    fn tree_debug_assert_passes() {
        tree().debug_assert();
    }

    fn parses(argv: &[&str]) -> bool {
        tree().try_get_matches_from(argv).is_ok()
    }

    #[test]
    fn parse_smoke() {
        assert!(parses(&["bob", "capture", "--route", "cash", "fix", "it"]));
        assert!(parses(&["bob", "capture", "--", "--2"]));
        assert!(parses(&["bob", "freshness", "-f", "json"]));
        assert!(parses(&["bob", "freshness", "list", "-f", "json"]));
        assert!(parses(&["bob", "vault-sync", "--dry-run"]));
        assert!(parses(&["bob", "gkeep", "pull", "--dry-run"]));
        assert!(parses(&["bob", "task", "archive", "-t", "10"]));
        assert!(parses(&["bob", "pomodoro", "notify", "-vv", "5", "10"]));
        assert!(parses(&["bob", "pomodoro", "-s"]));
        assert!(parses(&["bob", "pomodoro", "status", "-s"]));
    }

    #[test]
    fn descriptor_names_match_canonical_paths() {
        for (path, leaf) in crate::runner::canonical_leaves() {
            let expected = format!("bob {}", path.join(" "));
            assert_eq!(
                leaf.native_command.command().get_name(),
                expected,
                "descriptor name for path {path:?}"
            );
        }
    }

    fn descriptor_flags(command: &ClapCommand) -> (Vec<char>, Vec<String>) {
        let mut shorts = Vec::new();
        let mut longs = Vec::new();
        for arg in command.get_arguments() {
            if let Some(short) = arg.get_short() {
                shorts.push(short);
            }
            if let Some(long) = arg.get_long() {
                longs.push(long.to_string());
            }
        }
        shorts.sort_unstable();
        longs.sort_unstable();
        (shorts, longs)
    }

    /// Every `-x` / `--long` token in help text. A `-` counts only when
    /// it starts an option: at the start of the text or preceded by a
    /// non-alphanumeric, so intra-word dashes in `tmux-pomodoro` or
    /// `move-done-tasks` are ignored.
    fn help_option_tokens(text: &str) -> (Vec<char>, Vec<String>) {
        let chars: Vec<char> = text.chars().collect();
        let mut shorts = Vec::new();
        let mut longs = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            if chars[index] != '-' {
                index += 1;
                continue;
            }
            let preceded_by_word =
                index > 0 && chars[index - 1].is_ascii_alphanumeric();
            if preceded_by_word {
                index += 1;
                continue;
            }
            if chars.get(index + 1) == Some(&'-') {
                let mut end = index + 2;
                while end < chars.len()
                    && (chars[end].is_ascii_alphanumeric() || chars[end] == '-')
                {
                    end += 1;
                }
                let valid = end > index + 2
                    && chars
                        .get(index + 2)
                        .is_some_and(|next| next.is_ascii_alphanumeric());
                if valid {
                    longs.push(chars[index + 2..end].iter().collect());
                }
                index = end.max(index + 2);
            } else if let Some(next) = chars.get(index + 1)
                && next.is_ascii_alphanumeric()
            {
                shorts.push(*next);
                index += 2;
            } else {
                index += 1;
            }
        }
        shorts.sort_unstable();
        shorts.dedup();
        longs.sort_unstable();
        longs.dedup();
        (shorts, longs)
    }

    fn assert_drift(descriptor: &ClapCommand, help: &str, label: &str) {
        let (desc_shorts, desc_longs) = descriptor_flags(descriptor);
        for short in &desc_shorts {
            assert!(
                help.contains(&format!("-{short}")),
                "{label}: descriptor -{short} missing from help_text"
            );
        }
        for long in &desc_longs {
            assert!(
                help.contains(&format!("--{long}")),
                "{label}: descriptor --{long} missing from help_text"
            );
        }
        let (help_shorts, help_longs) = help_option_tokens(help);
        for short in &help_shorts {
            assert!(
                desc_shorts.contains(short),
                "{label}: help -{short} missing from descriptor"
            );
        }
        for long in &help_longs {
            assert!(
                desc_longs.contains(long),
                "{label}: help --{long} missing from descriptor"
            );
        }
    }

    #[test]
    fn hand_descriptor_drift() {
        assert_drift(
            &crate::native::collect_done::completion_descriptor(),
            &crate::native::collect_done::help_text(),
            "bob task archive",
        );
        assert_drift(
            &crate::native::nightly::completion_descriptor(),
            &crate::native::nightly::help_text(),
            "nightly",
        );
        assert_drift(
            &crate::native::notify::completion_descriptor(),
            &crate::native::notify::help_text(),
            "bob pomodoro notify",
        );
        assert_drift(
            &crate::native::pomodoro::completion_descriptor(),
            &crate::native::pomodoro::help_text(),
            "bob pomodoro status",
        );
        assert_drift(
            &crate::native::pomodoro::tmux_completion_descriptor(),
            &crate::native::pomodoro::tmux_help_text(),
            "bob pomodoro tmux",
        );
    }
}
