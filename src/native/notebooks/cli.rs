//! `bob notebooks list` and `check` commands.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{output, scan};
use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob notebooks";
const SCHEMA_VERSION: u32 = 1;

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    code
}

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_list();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_list(&matches)
}

pub(crate) fn run_check(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_check();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    execute_check(&matches)
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new("bob notebooks list")
        .about("List notebooks grouped by root (default)")
        .long_about(
            "Tree of notebooks grouped by root, with titles, summaries, counts, and issue markers. This command is read-only. Bare `bob notebooks` runs this command.",
        )
        .after_help(
            "Examples:\n  bob notebooks\n  bob notebooks list\n  bob notebooks list -r dev -f json",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(root_arg())
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob notebooks check")
        .about("Check notebooks against the contract")
        .long_about(
            "Findings grouped by notebook, plus a notebook/root/error/warning summary. Exits 1 on any error, 0 otherwise. This command is read-only.",
        )
        .after_help("Examples:\n  bob notebooks check\n  bob notebooks check -f json")
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .default_value("human")
        .value_parser(["human", "json"])
        .help("Output format: human or json")
}

fn root_arg() -> Arg {
    Arg::new("root")
        .long("root")
        .short('r')
        .value_name("ROOT")
        .help("Filter to one computed root identity")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn vault_from(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn format_is_json(matches: &ArgMatches) -> bool {
    matches.get_one::<String>("format").map(String::as_str) == Some("json")
}

fn root_filter(matches: &ArgMatches) -> Option<String> {
    matches
        .get_one::<String>("root")
        .map(|value| normalize_root(value))
}

fn normalize_root(value: &str) -> String {
    let trimmed = value.trim();
    let without_md = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".MD"))
        .unwrap_or(trimmed);
    without_md.to_string()
}

fn execute_list(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let root = root_filter(matches);
    let output = scan::scan_notebooks(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    let mut report = output.report;
    if let Some(root) = root {
        report
            .notebooks
            .retain(|entry| entry.root.as_deref() == Some(root.as_str()));
        // Recompute counts for the filtered set.
        let roots = report
            .notebooks
            .iter()
            .filter_map(|entry| entry.root.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let errors = report
            .notebooks
            .iter()
            .map(|entry| {
                entry
                    .issues
                    .iter()
                    .filter(|issue| {
                        issue.severity == super::model::Severity::Error
                    })
                    .count()
            })
            .sum();
        let warnings = report
            .notebooks
            .iter()
            .map(|entry| {
                entry
                    .issues
                    .iter()
                    .filter(|issue| {
                        issue.severity == super::model::Severity::Warning
                    })
                    .count()
            })
            .sum();
        report.counts = super::model::NotebookCounts {
            notebooks: report.notebooks.len(),
            roots,
            errors,
            warnings,
        };
    }
    if !output.io_errors.is_empty() {
        // Never present I/O failures as a clean scan.
        let payload = report.to_json();
        println!("{payload}");
        return 1;
    }
    if json {
        println!("{}", report.to_json());
        0
    } else {
        output::print_list(&report, &bob_dir);
        0
    }
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let output = scan::scan_notebooks(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    let report = output.report;
    if !output.io_errors.is_empty() {
        let payload = report.to_json();
        println!("{payload}");
        return 1;
    }
    if json {
        println!("{}", report.to_json());
    } else {
        output::print_check(&report);
    }
    if report.counts.errors > 0 {
        1
    } else {
        0
    }
}

#[allow(dead_code)]
fn schema_version() -> u32 {
    SCHEMA_VERSION
}
