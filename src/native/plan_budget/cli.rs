//! Read-only `bob plan` CLI: today's plan budget and this week's
//! NOW count, as human output or JSON.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use super::{
    super::{
        config::{self, ConfigError, PlanConfig},
        env as bob_env, pomodoro,
        style::Styler,
    },
    assemble_report, compute, count_now, LedgerBudget, NowBudget, PlanReport,
    PlanStatus,
};

const COMMAND_NAME: &str = "bob plan";

/// Bump only for a breaking change to the JSON object below; new
/// optional fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = PlanRequest::from_matches(&matches);

    match show_plan(&request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Show today's plan budget and this week's NOW count")
        .long_about(
            "Show today's Pomodoro plan budget from the daily note ledger \
            and this week's NOW count from the Tasks index.\n\n\
            The command is read-only. Themes are the distinct open Pomodoro \
            names besides the exempt ones; links are the distinct Task Links \
            under them; NOW counts open #now tasks visible today. See \
            docs/plan.md for the full definition. A missing daily note or \
            missing Pomodoros section still reports NOW and exits 0.",
        )
        .after_help(
            "Examples:\n  bob plan\n  bob plan -f json\n  bob plan -b ~/bob -f json\n\nEnvironment:\n  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for default daily-note selection\n  NO_COLOR                  Disable colored output",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanRequest {
    bob_dir: PathBuf,
}

impl PlanRequest {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: matches
                .get_one::<OsString>("bob-dir")
                .map(PathBuf::from)
                .map(|path| bob_env::expand_tilde(&path))
                .unwrap_or_else(bob_env::bob_dir),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanSuccess {
    report: PlanReport,
    no_daily_note: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanError {
    Io(String),
    InvalidConfig(String),
}

fn show_plan(request: &PlanRequest) -> Result<PlanSuccess, PlanError> {
    let config =
        config::load_plan_config(&config::config_path()).map_err(|error| {
            match error {
                ConfigError::Read(message) => PlanError::Io(message),
                ConfigError::Invalid(message) => {
                    PlanError::InvalidConfig(message)
                }
            }
        })?;

    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let relative_day_file = relative_day_file(&day_file, &request.bob_dir);
    let today = day_date(&day_file);
    let now = count_now(&request.bob_dir, today, &config);

    let contents = match fs::read_to_string(&day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PlanSuccess {
                report: report_without_ledger(
                    today,
                    &relative_day_file,
                    &config,
                    now,
                ),
                no_daily_note: true,
            });
        }
        Err(error) => {
            return Err(PlanError::Io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let ledger = compute(&contents, &config);
    Ok(PlanSuccess {
        report: assemble_report(
            today,
            &relative_day_file,
            &config,
            &ledger,
            now,
        ),
        no_daily_note: false,
    })
}

fn day_date(day_file: &Path) -> NaiveDate {
    if let Some(file_name) = day_file.file_name().and_then(|name| name.to_str())
        && let Some(date) = pomodoro::parse_day_file_date(file_name)
    {
        return date;
    }

    bob_env::current_datetime().date()
}

fn relative_day_file(day_file: &Path, bob_dir: &Path) -> String {
    day_file
        .strip_prefix(bob_dir)
        .unwrap_or(day_file)
        .display()
        .to_string()
        .trim_start_matches('/')
        .to_string()
}

fn report_without_ledger(
    today: NaiveDate,
    relative_day_file: &str,
    config: &PlanConfig,
    now: NowBudget,
) -> PlanReport {
    let ledger = LedgerBudget {
        has_section: false,
        themes: super::PlanMeter {
            count: 0,
            cap: config.max_themes(),
            over: false,
        },
        links: super::PlanMeter {
            count: 0,
            cap: config.max_links(),
            over: false,
        },
        status: PlanStatus::Ok,
        theme_names: Vec::new(),
        entries: Vec::new(),
        warnings: Vec::new(),
    };
    assemble_report(today, relative_day_file, config, &ledger, now)
}

fn print_success(result: &PlanSuccess, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => {
            let styler = Styler::detect();
            print!("{}", human_success(result, &styler));
        }
        OutputFormat::Json => {
            let mut value =
                serde_json::to_value(&result.report).unwrap_or(json!({}));
            value["ok"] = json!(true);
            value["schema_version"] = json!(SCHEMA_VERSION);
            println!("{value}");
        }
    }
}

fn human_success(result: &PlanSuccess, styler: &Styler) -> String {
    let report = &result.report;
    let weekday = report
        .date
        .parse::<NaiveDate>()
        .map(|date| date.format("%a").to_string())
        .unwrap_or_default();
    let mut output = String::new();
    if result.no_daily_note {
        output.push_str(&format!(
            "bob plan {sep} {weekday} {date} {sep} no daily note yet\n",
            sep = styler.separator(),
            date = report.date,
        ));
    } else if report.entries.is_empty() && report.theme_names.is_empty() {
        output.push_str(&format!(
            "bob plan {sep} {weekday} {date} {sep} no Pomodoros section\n",
            sep = styler.separator(),
            date = report.date,
        ));
    } else {
        output.push_str(&format!(
            "bob plan {sep} {weekday} {date} {sep} {file}\n",
            sep = styler.separator(),
            date = report.date,
            file = report.daily_file,
        ));
    }
    output.push('\n');

    let themes_meter =
        format!("{}/{} themes", report.themes.count, report.themes.cap);
    let links_meter =
        format!("{}/{} links", report.links.count, report.links.cap);
    let now_meter = format!("{}/{}", report.now.count, report.now.cap);
    let paint_meter = |styler: &Styler, text: &str, over: bool| {
        if over {
            styler.red(text)
        } else {
            styler.green(text)
        }
    };
    output.push_str(&format!(
        "  PLAN  {} {sep} {}        NOW  {}\n",
        paint_meter(styler, &themes_meter, report.themes.over),
        paint_meter(styler, &links_meter, report.links.over),
        paint_meter(styler, &now_meter, report.now.over),
        sep = styler.separator(),
    ));
    output.push('\n');

    if !report.entries.is_empty() {
        let name_width = report
            .entries
            .iter()
            .map(|entry| entry.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(5);
        let time_width = report
            .entries
            .iter()
            .filter_map(|entry| entry.time_range.as_deref())
            .map(|range| range.len())
            .max()
            .unwrap_or(0)
            + "▶ ".chars().count();
        for entry in &report.entries {
            let star = if entry.highlight {
                styler.yellow("★")
            } else {
                " ".to_string()
            };
            let time_cell = match entry.time_range.as_deref() {
                Some(range) => {
                    let cell = format!("{} {range}", styler.cyan("▶"));
                    let missing = time_width
                        .saturating_sub("▶ ".chars().count() + range.len());
                    format!("{cell}{}", " ".repeat(missing))
                }
                None if entry.exempt => styler.dim(&pad("exempt", time_width)),
                None => " ".repeat(time_width),
            };
            let name = pad(&entry.name, name_width);
            let links = if entry.links == 1 {
                "1 link".to_string()
            } else {
                format!("{} links", entry.links)
            };
            let row = format!("  {star} {name}  {time_cell}  {links}\n");
            output.push_str(&if entry.exempt { styler.dim(&row) } else { row });
        }
        output.push('\n');
    }

    for warning in &report.warnings {
        let mut line = format!(
            "  {} {}",
            styler.warning_prefix(),
            styler.yellow(&warning.message)
        );
        if let Some(number) = warning.line {
            line.push_str(&format!(" (line {number})"));
        }
        line.push_str(&format!("  {}\n", styler.dim(&warning.code)));
        output.push_str(&line);
    }

    output
}

fn pad(text: &str, width: usize) -> String {
    let missing = width.saturating_sub(text.chars().count());
    format!("{text}{}", " ".repeat(missing))
}

fn print_error(error: PlanError, output_format: OutputFormat) -> i32 {
    match error {
        PlanError::Io(message) => {
            if output_format == OutputFormat::Json {
                println!(
                    "{}",
                    json!({
                        "ok": false,
                        "schema_version": SCHEMA_VERSION,
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob plan: {message}");
            }
            1
        }
        PlanError::InvalidConfig(message) => {
            if output_format == OutputFormat::Json {
                println!(
                    "{}",
                    json!({
                        "ok": false,
                        "schema_version": SCHEMA_VERSION,
                        "error": message,
                    })
                );
            } else {
                eprintln!("bob plan: invalid plan config: {message}");
            }
            2
        }
    }
}
