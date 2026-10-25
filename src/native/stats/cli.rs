//! `bob stats` overview, show, and metrics: human, JSON, and CSV.

use std::{ffi::OsString, iter, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::{
    compute::compute,
    error::StatsError,
    render,
    request::{Grouping, MetricSelection, StatsRequest},
};
use crate::native::{config, env as bob_env, style::Styler};

const OVERVIEW_NAME: &str = "bob stats overview";
const SHOW_NAME: &str = "bob stats show";
const METRICS_NAME: &str = "bob stats metrics";

const OVERVIEW_AFTER_HELP: &str = "\
Examples:
  bob stats
  bob stats -r 7d -c
  bob stats -f json
  bob stats -f csv

Environment:
  BOB_CONFIG_FILE  Exact Bob config file; defaults to ~/.config/bob/config.yml
  BOB_DIR          Bob vault root when --bob-dir is omitted
  BOB_NOW          Local datetime override that pins today for the report
  NO_COLOR         Disable colored output

Exit codes:
  0  report
  1  vault I/O failure
  2  usage, unknown metric, config error, or non-Dataview task format

See docs/stats.md for the catalog, JSON contract, and display rules.";

const SHOW_AFTER_HELP: &str = "\
Examples:
  bob stats show worked -c
  bob stats show worked reviewed
  bob stats show -a -f json
  bob stats show -a -f csv

Environment:
  BOB_CONFIG_FILE  Exact Bob config file; defaults to ~/.config/bob/config.yml
  BOB_DIR          Bob vault root when --bob-dir is omitted
  BOB_NOW          Local datetime override that pins today for the report
  NO_COLOR         Disable colored output

Exit codes:
  0  report
  1  vault I/O failure
  2  usage, unknown metric, config error, or non-Dataview task format

See docs/stats.md for the catalog, JSON contract, and display rules.";

const METRICS_AFTER_HELP: &str = "\
Examples:
  bob stats metrics
  bob stats metrics -f json

Environment:
  BOB_CONFIG_FILE  Exact Bob config file; defaults to ~/.config/bob/config.yml
  BOB_DIR          Bob vault root when --bob-dir is omitted
  BOB_NOW          Local datetime override that pins today for the report
  NO_COLOR         Disable colored output

Exit codes:
  0  report
  1  vault I/O failure
  2  usage, unknown metric, config error, or non-Dataview task format

See docs/stats.md for the catalog, JSON contract, and display rules.";

pub(crate) fn run_overview(args: Vec<OsString>) -> i32 {
    run_named(OVERVIEW_NAME, overview_command(), args, View::Overview)
}

pub(crate) fn run_show(args: Vec<OsString>) -> i32 {
    run_named(SHOW_NAME, show_command(), args, View::Show)
}

pub(crate) fn run_metrics(args: Vec<OsString>) -> i32 {
    run_named(METRICS_NAME, metrics_command(), args, View::Metrics)
}

fn run_named(
    command_name: &str,
    mut command: ClapCommand,
    args: Vec<OsString>,
    view: View,
) -> i32 {
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(command_name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error, command_name),
    };
    let format = OutputFormat::from_matches(&matches);
    let request = match request_from_matches(&matches, view) {
        Ok(request) => request,
        Err(error) => return print_stats_error(&error, format, command_name),
    };
    match compute(&request) {
        Ok(report) => {
            print_report(&report, view, format);
            0
        }
        Err(error) => print_stats_error(&error, format, command_name),
    }
}

fn print_clap_error(error: clap::Error, command_name: &str) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of reporting
        // the failed render, but keep clap's own exit code so a usage error
        // never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{command_name}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

fn print_stats_error(
    error: &StatsError,
    format: OutputFormat,
    command_name: &str,
) -> i32 {
    match format {
        OutputFormat::Json => {
            let line =
                serde_json::to_string(&error.envelope()).unwrap_or_else(|_| {
                    "{\"ok\":false,\"schema_version\":1,\"error\":{\"code\":\
                     \"encode\",\"message\":\"failed to encode stats error\"}}"
                        .to_string()
                });
            println!("{line}");
        }
        OutputFormat::Human | OutputFormat::Csv => {
            eprintln!("{command_name}: {}", error.message());
        }
    }
    error.exit_code()
}

fn print_report(
    report: &super::report::StatsReport,
    view: View,
    format: OutputFormat,
) {
    let styler = Styler::detect();
    let text = match (view, format) {
        (_, OutputFormat::Json) if matches!(view, View::Metrics) => {
            render::catalog_json(report)
        }
        (_, OutputFormat::Json) => render::json_line(report),
        (_, OutputFormat::Csv) => render::csv(report),
        (View::Overview, OutputFormat::Human) => {
            render::overview(report, &styler)
        }
        (View::Show, OutputFormat::Human) => render::detail(report, &styler),
        (View::Metrics, OutputFormat::Human) => {
            render::catalog(report, &styler)
        }
    };
    if text.ends_with('\n') {
        print!("{text}");
    } else {
        println!("{text}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Show,
    Metrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
    Csv,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            "csv" => Self::Csv,
            _ => Self::Human,
        }
    }
}

fn request_from_matches(
    matches: &ArgMatches,
    view: View,
) -> Result<StatsRequest, StatsError> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let mut request = StatsRequest::new(vault, config::config_path());
    request.selection = selection_from_matches(matches, view)?;
    request.range = matches
        .try_get_one::<String>("range")
        .ok()
        .flatten()
        .cloned();
    request.grouping = grouping_from_matches(matches);
    request.offset = matches
        .try_get_one::<u32>("offset")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(0);
    request.compare = match view {
        View::Metrics => false,
        View::Overview | View::Show => matches.get_flag("compare"),
    };
    Ok(request)
}

fn selection_from_matches(
    matches: &ArgMatches,
    view: View,
) -> Result<MetricSelection, StatsError> {
    match view {
        View::Overview => Ok(MetricSelection::Overview),
        View::Metrics => Ok(MetricSelection::AllVisible),
        View::Show => {
            if matches.get_flag("all") {
                return Ok(MetricSelection::AllVisible);
            }
            let ids: Vec<String> = matches
                .get_many::<String>("metrics")
                .map(|values| values.cloned().collect())
                .unwrap_or_default();
            if ids.is_empty() {
                return Err(StatsError::usage("pass a METRIC or -a/--all"));
            }
            Ok(MetricSelection::Explicit(ids))
        }
    }
}

fn grouping_from_matches(matches: &ArgMatches) -> Grouping {
    match matches
        .try_get_one::<String>("group")
        .ok()
        .flatten()
        .map(String::as_str)
        .unwrap_or("auto")
    {
        "day" => Grouping::Day,
        "week" => Grouping::Week,
        "month" => Grouping::Month,
        _ => Grouping::Auto,
    }
}

pub(crate) fn overview_command() -> ClapCommand {
    ClapCommand::new(OVERVIEW_NAME)
        .about("Show the weekly glance (default)")
        .long_about(
            "Show the weekly glance of overview metrics as sparklines, \
            today's in-progress values, and journal health. Bare \
            `bob stats` runs this command. The command is read-only. \
            See docs/stats.md for the catalog and display rules.",
        )
        .after_help(OVERVIEW_AFTER_HELP)
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(compare_arg())
        .arg(format_arg(true))
        .arg(group_arg())
        .arg(help_arg())
        .arg(offset_arg())
        .arg(range_arg())
}

pub(crate) fn show_command() -> ClapCommand {
    ClapCommand::new(SHOW_NAME)
        .about("Show one or more metrics in detail")
        .long_about(
            "Show one or more metrics as a detail chart (or weekday \
            grid for booleans), with optional prior-period ticks. \
            Pass metric ids or -a/--all for every visible metric. \
            The command is read-only. See docs/stats.md.",
        )
        .after_help(SHOW_AFTER_HELP)
        .disable_help_flag(true)
        .arg(metrics_arg())
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(compare_arg())
        .arg(format_arg(true))
        .arg(group_arg())
        .arg(help_arg())
        .arg(offset_arg())
        .arg(range_arg())
}

pub(crate) fn metrics_command() -> ClapCommand {
    ClapCommand::new(METRICS_NAME)
        .about("List metric ids, kinds, and definitions")
        .long_about(
            "List every visible metric: id, label, kind, unit, source, \
            origin, since, and a dim definition line. JSON prints full \
            definitions and parameters without points. The command is \
            read-only. See docs/stats.md.",
        )
        .after_help(METRICS_AFTER_HELP)
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg(false))
        .arg(help_arg())
}

fn metrics_arg() -> Arg {
    Arg::new("metrics")
        .value_name("METRIC")
        .num_args(1..)
        .required_unless_present("all")
        .help("Metric id (repeatable)")
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("Show every visible metric")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn compare_arg() -> Arg {
    Arg::new("compare")
        .long("compare")
        .short('c')
        .action(ArgAction::SetTrue)
        .help("Include the prior period's points")
}

fn format_arg(csv: bool) -> Arg {
    let arg = Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .default_value("human");
    if csv {
        arg.value_parser(["human", "json", "csv"])
            .help("Output format: human, json, or csv")
    } else {
        arg.value_parser(["human", "json"])
            .help("Output format: human or json")
    }
}

fn group_arg() -> Arg {
    Arg::new("group")
        .long("group")
        .short('g')
        .value_name("GROUP")
        .value_parser(["auto", "day", "week", "month"])
        .default_value("auto")
        .help("Bucket points by auto, day, week, or month")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Print help")
}

fn offset_arg() -> Arg {
    Arg::new("offset")
        .long("offset")
        .short('o')
        .value_name("N")
        .value_parser(clap::value_parser!(u32))
        .help("Shift the window back this many ranges")
}

fn range_arg() -> Arg {
    Arg::new("range")
        .long("range")
        .short('r')
        .value_name("RANGE")
        .help("Window as 7d, 4w, 3m, 1y, all, or START..END")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_parses_format_json() {
        let matches = overview_command()
            .try_get_matches_from(vec![OVERVIEW_NAME, "-f", "json"])
            .unwrap();
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Json);
    }

    #[test]
    fn show_requires_metric_or_all() {
        let error = show_command()
            .try_get_matches_from(vec![SHOW_NAME])
            .unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn show_all_skips_metric() {
        show_command()
            .try_get_matches_from(vec![SHOW_NAME, "-a"])
            .unwrap();
    }

    #[test]
    fn metrics_rejects_csv() {
        let error = metrics_command()
            .try_get_matches_from(vec![METRICS_NAME, "-f", "csv"])
            .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
    }
}
