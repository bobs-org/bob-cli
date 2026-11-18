//! Read-only `bob walks`: s/w/b attention as human output or JSON.
//!
//! The contract lives in `docs/walks.md`. Exit 0 whenever the report
//! prints, including unavailable families. Nonzero only for clap
//! usage errors or an unusable vault root.

use std::{ffi::OsString, iter, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use super::super::{env as bob_env, style::Styler};
use super::{build_report, FamilyReport, VaultError};

const COMMAND_NAME: &str = "bob walks";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    let bob_dir = bob_dir_from_matches(&matches);
    match build_report(&bob_dir) {
        Ok(report) => {
            match format {
                OutputFormat::Human => {
                    let styler = Styler::detect();
                    print!("{}", human_report(&report.walks, &styler));
                }
                OutputFormat::Json => {
                    println!("{}", report.to_json());
                }
            }
            0
        }
        Err(error) => print_vault_error(&error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn print_vault_error(error: &VaultError, format: OutputFormat) -> i32 {
    let message = error.message();
    match format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {message}");
        }
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }));
        }
    }
    1
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Show which bracket walks need attention: s review, w lanes, b bank")
        .long_about(
            "Show which bracket walks need attention. The report has one \
            row per family: s (any review row ]s can land on), w (lanes over cap), \
            and b (waiting bank rows). Each family is computed on its \
            own, so a failure in one still reports the others.\n\n\
            The command is read-only: it never stamps a task, never \
            walks a queue, and never writes the vault. See docs/walks.md \
            for the predicates, labels, and JSON schema.",
        )
        .after_help(
            "Examples:\n  bob walks\n  bob walks -f json\n  bob walks -b ~/bob -f json\n\nEnvironment:\n  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for review date selection\n  NO_COLOR                  Disable colored output",
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
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

pub(crate) fn human_report(
    walks: &[FamilyReport; 3],
    styler: &Styler,
) -> String {
    let mut output = String::new();
    for family in walks {
        output.push_str(&human_line(family, styler));
        output.push('\n');
    }
    output
}

fn human_line(family: &FamilyReport, styler: &Styler) -> String {
    let letter = family.family;
    if !family.available {
        let reason = family.reason.as_deref().unwrap_or("unavailable");
        return format!(
            "{letter} {}",
            styler.dim(&format!("? unavailable — {reason}"))
        );
    }
    if family.attention {
        let bullet = attention_bullet(styler, family.tier.unwrap_or(""));
        let label = family.label.as_deref().unwrap_or("");
        format!("{letter} {bullet} {label}")
    } else {
        format!(
            "{letter} {}",
            styler.dim(&format!("· {}", calm_text(family.family)))
        )
    }
}

fn calm_text(family: &str) -> &'static str {
    match family {
        "s" => "Review clear",
        "w" => "Lanes within limits",
        "b" => "Bank clear",
        _ => "clear",
    }
}

fn attention_bullet(styler: &Styler, tier: &str) -> String {
    let code = match tier {
        "pre" | "post" => "36;1",
        "new" | "over" => "31;1",
        "projects" | "recurring" => "35;1",
        "pending" | "waiting" => "33;1",
        "next" => "32;1",
        "tickler" => "34;1",
        "rotten" => "38;5;208;1",
        _ => "1",
    };
    styler.paint(code, "●")
}
