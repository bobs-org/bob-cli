//! `bob periods` CLI wiring: the Vault group for period notes.
//!
//! The group owns `ensure`, `list`, and `migrate`. Later phases add
//! `show`, `brief`, and `draft`. Options stay alphabetical and every
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
             deterministic scaffold; existing notes that are empty, \
             whitespace-only, or only frontmatter (the stub a clicked \
             dangling link leaves) are filled. A note with body content is \
             never touched. A re-run writes nothing. With -R/--refresh-instructions, \
             stale agent callouts in existing notes are refreshed in one \
             commit instead. See docs/periods.md for the calendar, scaffolds, \
             and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob periods ensure\n  \
             bob periods ensure -d 2026-12-28\n  \
             bob periods ensure -n -f json\n  \
             bob periods ensure -q\n  \
             bob periods ensure -R\n\n\
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
        .arg(refresh_instructions_arg())
}

pub(crate) fn list_command() -> ClapCommand {
    ClapCommand::new("bob periods list")
        .about("List period notes with review states (default)")
        .long_about(
            "List the period notes that exist, newest first, grouped year → \
             quarter → weeks. Each row shows its review-state mark and its \
             Headline. -p/--problems reports dangling daily parents, missing \
             period notes, unscaffolded notes, wrong parents, task rows, and \
             stale instructions. The command is read-only. See docs/periods.md \
             for the review states, problems, and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob periods\n  \
             bob periods list\n  \
             bob periods list -k week -y 2026\n  \
             bob periods list -p\n  \
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
        .arg(problems_arg())
        .arg(year_arg())
}

pub(crate) fn brief_command() -> ClapCommand {
    ClapCommand::new("bob periods brief")
        .about("Print the agent review bundle for a week, quarter, or year")
        .long_about(
            "Print the deterministic, versioned agent bundle (brief v2) for \
             a week, quarter, or year: header, facts, time, parts, finished, \
             started and dropped, still open, practice (year only), Bryan's \
             words, this note and its neighbours, off-clock counts, and \
             caveats. The command is read-only. all_time has no review. See \
             docs/periods.md for the brief contract and token budgets.",
        )
        .after_help(
            "Examples:\n  \
             bob periods brief 2026-W45\n  \
             bob periods brief 2026-Q4 -f json\n  \
             bob periods brief 2026 -f md\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_NOW                   Local datetime override that pins today\n  \
             BOB_CONFIG_FILE           Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  brief\n  \
             1  vault I/O failure\n  \
             2  usage (bad id or flags), or all_time (which has no review)\n\n\
             See docs/periods.md for the brief contract and token budgets.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(brief_format_arg())
        .arg(help_arg())
        .arg(Arg::new("period").value_name("PERIOD").help(
            "Period id or note path (week, quarter, year, last-week, last-quarter, or last-year); all_time has no review",
        ))
}

pub(crate) fn draft_command() -> ClapCommand {
    ClapCommand::new("bob periods draft")
        .about("Draft a period's Review section (never overwrites Bryan)")
        .long_about(
            "Replace the `## Review` section of a week, quarter, or year note \
             with the draft text from -i/--input (a file path, or - for \
             stdin). The text must open with a `**Headline:**` paragraph \
             (joined onto one line on save) and stay within the kind's word \
             cap (week 250, quarter 400, year 600). \
             An open or future period, or a Review Bryan already edited or \
             reflected, is refused unless -F/--force is passed. The note must \
             exist and contain `## Review`; all_time has no review. See \
             docs/periods.md for the draft rules and JSON errors.",
        )
        .after_help(
            "Examples:\n  \
             bob periods draft 2026-W45 -i /tmp/review.md\n  \
             bob periods draft 2026-Q4 -i - < review.md\n  \
             bob periods draft 2026-W45 -i review.md -n\n  \
             bob periods draft 2026-W45 -i review.md -F\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_NOW                   Local datetime override that pins today\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  drafted (or planned with -n)\n  \
             1  vault I/O or transaction failure\n  \
             2  usage (bad id, flags, or draft text)\n  \
             3  refused (period open, or state edited/reflected)\n\n\
             See docs/periods.md for the draft rules, word caps, and JSON errors.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(draft_force_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(draft_input_arg())
        .arg(dry_run_arg())
        .arg(Arg::new("period").value_name("PERIOD").help(
            "Period id or note path (week, quarter, year, last-week, last-quarter, or last-year); all_time has no review",
        ))
}

pub(crate) fn migrate_command() -> ClapCommand {
    ClapCommand::new("bob periods migrate")
        .about("Adopt the period-note tree (dry run by default)")
        .long_about(
            "Adopt the period-note tree: create all_time.md and the ISO \
             week/quarter/year notes in range, adopt in-range year notes \
             (keeping zorg_* keys and archiving the old body), re-parent \
             out-of-range year notes and canonical daily notes, and add \
             the week link on standard prev/next nav lines. Dry run by \
             default. -a/--apply runs one verified vault-rewrite commit. \
             See docs/periods.md for the range, verify, and JSON.",
        )
        .after_help(
            "Examples:\n  \
             bob periods migrate\n  \
             bob periods migrate -s 2025-12-29\n  \
             bob periods migrate -a\n  \
             bob periods migrate -a -o\n  \
             bob periods migrate -a -f json\n\n\
             Environment:\n  \
             BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
             BOB_NOW                   Local datetime override that pins today\n  \
             BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to ${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock\n  \
             BOB_CONFIG_FILE           Stats config used by the apply verify\n  \
             NO_COLOR                  Disable colored output\n\n\
             Exit codes:\n  \
             0  planned or adopted\n  \
             1  vault I/O or transaction failure\n  \
             2  usage error\n  \
             3  refused (not a worktree, dirty paths)\n\n\
             See docs/periods.md for the range, verify, and JSON.",
        )
        .disable_help_flag(true)
        .arg(apply_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(offline_arg())
        .arg(since_arg())
}

pub(crate) fn show_command() -> ClapCommand {
    ClapCommand::new("bob periods show")
        .about("Show a period's live stats and child summaries")
        .long_about(
            "Show the stats strip and parts-table data for a week, quarter, \
             year, or all time. With no PERIOD, show the current ISO week. \
             The command is read-only. See docs/periods.md for the JSON \
             contract and aggregation rules.",
        )
        .after_help(
            "Examples:\n  \
             bob periods show\n  \
             bob periods show 2026-W45 -f json\n  \
             bob periods show 2026-Q4 -k total\n  \
             bob periods show all_time -f json\n\n\
             Environment:\n  \
             BOB_CONFIG_FILE  Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
             BOB_DIR          Bob vault root; defaults to ~/bob\n  \
             BOB_NOW          Local datetime override that pins today for the report\n  \
             NO_COLOR         Disable colored output\n\n\
             Exit codes:\n  \
             0  report\n  \
             1  vault I/O failure\n  \
             2  usage, unknown period, config error, or non-Dataview task format\n\n\
             See docs/periods.md for the calendar, JSON contract, and display rules.",
        )
        .disable_help_flag(true)
        .arg(show_clock_arg())
        .arg(show_format_arg())
        .arg(help_arg())
        .arg(Arg::new("period").value_name("PERIOD").help(
            "Period id or note path (week, quarter, year, last-week, last-quarter, or last-year); defaults to the current week",
        ))
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

/// `brief` prints Markdown for agents, so its human format is `md`; the
/// agent callouts run `bob periods brief <id> -f md`. `human` stays a
/// hidden alias for symmetry with the other period commands.
fn brief_format_arg() -> Arg {
    use clap::builder::{PossibleValue, PossibleValuesParser};
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(PossibleValuesParser::new([
            PossibleValue::new("md").alias("human"),
            PossibleValue::new("json"),
        ]))
        .default_value("md")
        .help("Output format: md (Markdown) or json")
}

fn show_format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn show_clock_arg() -> Arg {
    use clap::builder::{PossibleValue, PossibleValuesParser};
    Arg::new("clock")
        .long("clock")
        .short('k')
        .value_name("CLOCK")
        .value_parser(PossibleValuesParser::new([
            PossibleValue::new("worked").alias("on"),
            PossibleValue::new("off-clock").aliases(["off", "untracked"]),
            PossibleValue::new("total").alias("all"),
        ]))
        .default_value("worked")
        .help("Pomodoro clock: worked, off-clock, or total")
}

fn draft_force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Draft an open period or overwrite Bryan's edits (Bryan said redo, or asked for so far)")
}

fn draft_input_arg() -> Arg {
    Arg::new("input")
        .long("input")
        .short('i')
        .value_name("FILE")
        .value_parser(OsStringValueParser::new())
        .help("Draft text file, or - for stdin (required)")
}

fn refresh_instructions_arg() -> Arg {
    Arg::new("refresh-instructions")
        .long("refresh-instructions")
        .short('R')
        .action(ArgAction::SetTrue)
        .help("Refresh stale agent callouts in existing period notes (one commit)")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn apply_arg() -> Arg {
    Arg::new("apply")
        .long("apply")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Write the plan and make one verified commit")
}

fn offline_arg() -> Arg {
    Arg::new("offline")
        .long("offline")
        .short('o')
        .action(ArgAction::SetTrue)
        .help("Commit locally; skip vault sync before and after")
}

fn kind_arg() -> Arg {
    Arg::new("kind")
        .long("kind")
        .short('k')
        .value_name("KIND")
        .value_parser(["week", "quarter", "year"])
        .help("Show only one period kind")
}

fn problems_arg() -> Arg {
    Arg::new("problems")
        .long("problems")
        .short('p')
        .action(ArgAction::SetTrue)
        .help("Report period-note problems (read-only)")
}

fn since_arg() -> Arg {
    Arg::new("since")
        .long("since")
        .short('s')
        .value_name("DATE")
        .help("First day of the adoption range, as YYYY-MM-DD; defaults to the ISO year start")
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
