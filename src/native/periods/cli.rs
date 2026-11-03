//! `bob periods` CLI wiring: the Vault group for period notes.
//!
//! The group owns `ensure` and `list` in core; later phases add `show`,
//! `brief`, `draft`, and `migrate`. Options stay alphabetical and every
//! long option keeps a short alias.

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, Command as ClapCommand,
};

pub(crate) fn ensure_command() -> ClapCommand {
    ClapCommand::new("bob periods ensure")
        .about("Create missing period notes one week ahead")
        .long_about(
            "Create missing period notes one week ahead: the week, quarter, \
             year, and all-time chains for today (or --date) and for that \
             date plus seven days. Missing notes are created from the \
             deterministic scaffold; existing notes that are empty or \
             whitespace-only are filled. A note with content is never \
             touched. A re-run writes nothing. See docs/periods.md for the \
             calendar, scaffolds, and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob periods ensure\n  \
             bob periods ensure -d 2026-12-28\n  \
             bob periods ensure -n -f json\n  \
             bob periods ensure -q\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_NOW                   Local datetime override that pins today\n  \
             BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  ensured\n  \
             1  vault I/O failure or lock busy\n  \
             2  usage error\n\n\
             See docs/periods.md for the calendar, scaffolds, and JSON.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(date_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(quiet_arg())
}

pub(crate) fn list_command() -> ClapCommand {
    ClapCommand::new("bob periods list")
        .about("List period notes with review states (default)")
        .long_about(
            "List the period notes that exist, newest first, grouped year → \
             quarter → weeks. Each row shows its review-state mark and its \
             Headline. The command is read-only. See docs/periods.md for the \
             review states and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob periods\n  \
             bob periods list\n  \
             bob periods list -k week -y 2026\n  \
             bob periods list -f json\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_NOW                   Local datetime override that pins today\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  list\n  \
             1  vault I/O failure\n  \
             2  usage error\n\n\
             See docs/periods.md for the review states and JSON.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(kind_arg())
        .arg(year_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn date_arg() -> Arg {
    Arg::new("date")
        .long("date")
        .short('d')
        .value_name("DATE")
        .help("ISO date to ensure for, as YYYY-MM-DD; defaults to today")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('n')
        .action(ArgAction::SetTrue)
        .help("Plan without writing files")
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

fn kind_arg() -> Arg {
    Arg::new("kind")
        .long("kind")
        .short('k')
        .value_name("KIND")
        .value_parser(["week", "quarter", "year"])
        .help("Show only one period kind")
}

fn quiet_arg() -> Arg {
    Arg::new("quiet")
        .long("quiet")
        .short('q')
        .action(ArgAction::SetTrue)
        .help("Print nothing unless files were written")
}

fn year_arg() -> Arg {
    Arg::new("year")
        .long("year")
        .short('y')
        .value_name("YEAR")
        .help("Show only periods in one ISO year, as YYYY")
}
