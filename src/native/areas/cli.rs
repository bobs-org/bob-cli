//! `bob areas` list and check commands.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::report;
use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob areas";

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
    ClapCommand::new("bob areas list")
        .about("List the area tree (default)")
        .long_about(
            "Colored area tree with hues, optional titles and summaries, and project/Today chips. This command is read-only and always exits 0. Bare `bob areas` runs this command.",
        )
        .after_help(
            "Examples:\n  bob areas\n  bob areas list\n  bob areas list -f json\n\nSee docs/areas.md for the area catalog contract.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
}

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob areas check")
        .about("Check areas against the contract")
        .long_about(
            "Findings grouped by path, plus an area/error/warning summary. Exits 1 on any error finding (including a damaged catalog), 0 when the vault is clean or warning-only. This command is read-only.",
        )
        .after_help(
            "Examples:\n  bob areas check\n  bob areas check -f json\n\nSee docs/areas.md for the area catalog contract.",
        )
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

fn execute_list(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let output = report::scan_areas(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    if !output.io_errors.is_empty() {
        println!("{}", output.report.to_json("list"));
        return 1;
    }
    if json {
        println!("{}", output.report.to_json("list"));
        0
    } else {
        super::output::print_list(&output.report);
        0
    }
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let output = report::scan_areas(&bob_dir);
    for error in &output.io_errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
    if !output.io_errors.is_empty() && !json {
        return 1;
    }
    if !output.io_errors.is_empty() {
        println!("{}", output.report.to_json("check"));
        return 1;
    }
    if json {
        println!("{}", output.report.to_json("check"));
    } else {
        super::output::print_check(&output.report);
    }
    if output.report.counts.errors > 0 {
        1
    } else {
        0
    }
}
