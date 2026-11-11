//! `bob bank check`.

use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{check, output, scan};
use crate::native::env as bob_env;

const COMMAND_NAME: &str = "bob bank";

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

pub(crate) fn build_cli_check() -> ClapCommand {
    ClapCommand::new("bob bank check")
        .about("Check bank notes against the contract")
        .long_about(
            "Findings grouped by note, then day, plus an error/warning summary. Exits 1 on any error, 0 otherwise. This command is read-only. With no bank.md and no bank/ folder, prints a teaching empty state and exits 0.",
        )
        .after_help(
            "Examples:\n  bob bank check\n  bob bank check -f json\n\nSee docs/bank.md for the bank contract.",
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

fn print_io_errors(errors: &[String]) {
    for error in errors {
        eprintln!("{COMMAND_NAME}: {error}");
    }
}

fn execute_check(matches: &ArgMatches) -> i32 {
    let bob_dir = vault_from(matches);
    let json = format_is_json(matches);
    let scan = scan::scan_bank(&bob_dir);
    print_io_errors(&scan.io_errors);
    if !scan.io_errors.is_empty() && !json {
        return 1;
    }
    let today = bob_env::current_datetime().date();
    let report = check::lint(&scan, today);
    if json {
        println!("{}", report.to_json());
    } else if scan.io_errors.is_empty() {
        output::print_check(&report);
    }
    report.exit_code()
}
