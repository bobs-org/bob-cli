//! `bob ref jobs` command group: `list` (bare default) plus `run`.
//!
//! A bare `bob ref jobs` lists recent jobs; it never clips or writes,
//! so it is the flag-only, read-only default the CLI rules require.

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::{
    output::{run_list, ListFormat},
    spool::{jobs_dir, list_jobs},
    worker::run_jobs,
};

/// The `jobs` group for `bob ref`: `list` plus `run`.
pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("jobs")
        .about("Queue and clip reading-queue links in the background")
        .subcommand(list_command())
        .subcommand(run_command())
        .subcommand_required(true)
        .arg_required_else_help(true)
        .after_help(
            "Examples:\n  bob ref jobs\n  bob ref jobs --all\n  bob ref jobs -f json\n  bob ref jobs run\n\nA bare `bob ref jobs` lists recent jobs; it never clips or writes.",
        )
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List recent ref jobs (default)")
        .arg(
            Arg::new("all")
                .long("all")
                .short('a')
                .action(ArgAction::SetTrue)
                .help("Show every recorded job, not just the last 7 days"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Print human text or machine-readable JSON"),
        )
        .after_help(
            "Examples:\n  bob ref jobs\n  bob ref jobs --all\n  bob ref jobs -f json",
        )
}

fn run_command() -> ClapCommand {
    ClapCommand::new("run")
        .about("Clip pending ref jobs through the shared ingest")
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .short('q')
                .action(ArgAction::SetTrue)
                .help("Suppress per-job lines; the summary still prints"),
        )
        .after_help(
            "Examples:\n  bob ref jobs run\n  bob ref jobs run -q\n\nA second worker exits 0 while the first holds the lock.",
        )
}

/// Dispatch `jobs list|run`. Returns the process exit code.
pub(crate) fn run(matches: &ArgMatches) -> i32 {
    match matches.subcommand() {
        Some(("list", sub_matches)) => {
            let view = list_jobs(&jobs_dir(), sub_matches.get_flag("all"));
            run_list(
                &view,
                sub_matches.get_flag("all"),
                ListFormat::parse(
                    sub_matches
                        .get_one::<String>("format")
                        .map(String::as_str)
                        .unwrap_or("human"),
                ),
            )
        }
        Some(("run", sub_matches)) => {
            run_jobs(&jobs_dir(), sub_matches.get_flag("quiet"))
        }
        _ => 2,
    }
}
