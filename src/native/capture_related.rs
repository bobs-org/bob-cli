//! Read-only similar-task matcher and `--query` evaluation harness.

mod bands;
mod corpus;
mod score;
#[cfg(test)]
mod tests;
mod tokenize;

use std::{
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{env as bob_env, style::Styler};

const COMMAND_NAME: &str = "bob capture-related";
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match RelatedRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(message) => return print_error(&message, output_format, 2),
    };

    match request {
        RelatedRequest::Query(query) => {
            let result = query_related(&query);
            print_success(&result, output_format);
            0
        }
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

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List existing tasks that a new capture may duplicate")
        .long_about(
            "List existing tasks that each new task in a capture draft may \
duplicate or relate to.\n\n\
The command is read-only and never writes the vault. This release ships \
the --query evaluation harness: TEXT is ranked as one new task title \
against every open task in routable notes, plus tasks closed in the last \
--days days. Pass --format json to emit schema_version 1 with a score on \
each match.",
        )
        .after_help(
            "Examples:\n  \
bob capture-related --query \"Install UPS battery\"\n  \
bob capture-related -q \"Install UPS battery\" -f json\n  \
bob capture-related -q \"Install UPS battery\" -d 14 -l 3\n  \
bob capture-related -b ~/bob -q \"Install UPS battery\"\n\n\
Environment:\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override for the closed-task window",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(days_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(limit_arg())
        .arg(query_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn days_arg() -> Arg {
    Arg::new("days")
        .long("days")
        .short('d')
        .value_name("N")
        .value_parser(clap::value_parser!(u32))
        .default_value("30")
        .help("Also match tasks closed in the last N days")
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

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('l')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .default_value("5")
        .help("Matches per item")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .short('q')
        .value_name("TEXT")
        .allow_hyphen_values(true)
        .required(true)
        .help("Rank TEXT as one new task title (evaluation harness)")
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
enum RelatedRequest {
    Query(QueryRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryRequest {
    bob_dir: PathBuf,
    days: u32,
    limit: usize,
    query: String,
}

impl RelatedRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, String> {
        let Some(query) = matches.get_one::<String>("query") else {
            return Err("pass --query TEXT to rank a task title".to_string());
        };
        Ok(Self::Query(QueryRequest {
            bob_dir: bob_dir_from_matches(matches),
            days: *matches.get_one::<u32>("days").unwrap_or(&30),
            limit: *matches.get_one::<usize>("limit").unwrap_or(&5),
            query: query.clone(),
        }))
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct CaptureRelatedResult {
    pub(super) ok: bool,
    pub(super) schema_version: u32,
    pub(super) warnings: Vec<String>,
    pub(super) items: Vec<RelatedItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct RelatedItem {
    pub(super) index: usize,
    pub(super) mode: String,
    pub(super) matches: Vec<RelatedMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct RelatedMatch {
    pub(super) key: String,
    pub(super) band: String,
    pub(super) title: String,
    pub(super) matched_ranges: Vec<(usize, usize)>,
    pub(super) kind: String,
    pub(super) status_symbol: String,
    pub(super) status_name: String,
    pub(super) status_type: String,
    pub(super) closed: Option<ClosedJson>,
    pub(super) route: String,
    pub(super) locator: String,
    pub(super) note_path: String,
    pub(super) target: String,
    pub(super) line: usize,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) block_id: Option<String>,
    pub(super) score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ClosedJson {
    pub(super) status: String,
    pub(super) date: String,
    pub(super) days_ago: i64,
}

fn query_related(request: &QueryRequest) -> CaptureRelatedResult {
    query_related_in(
        &request.bob_dir,
        &request.query,
        request.days,
        request.limit,
    )
}

pub(super) fn query_related_in(
    bob_dir: &Path,
    query: &str,
    days: u32,
    limit: usize,
) -> CaptureRelatedResult {
    let discovery = corpus::discover(bob_dir, days);
    let title = tokenize::display_title(query, &discovery.global_filter, None);
    let query_tokens = tokenize::tokenize(&title);
    let scores = score::cosine_scores(&query_tokens, &discovery.candidates);
    let ranked = bands::rank(
        &title,
        &query_tokens,
        &discovery.candidates,
        &scores,
        None,
        limit,
    );
    let matches = ranked
        .into_iter()
        .map(|ranked| RelatedMatch {
            key: ranked.candidate.key.clone(),
            band: ranked.band.as_str().to_string(),
            title: ranked.candidate.title.clone(),
            matched_ranges: ranked.matched_ranges,
            kind: ranked.candidate.kind_name().to_string(),
            status_symbol: ranked.candidate.status_symbol.to_string(),
            status_name: ranked.candidate.status_name.clone(),
            status_type: ranked.candidate.status_type.to_string(),
            closed: ranked.candidate.closed.as_ref().map(|closed| ClosedJson {
                status: closed.status.as_str().to_string(),
                date: task_fields_date(closed.date),
                days_ago: closed.days_ago,
            }),
            route: ranked.candidate.route.clone(),
            locator: ranked.candidate.locator.clone(),
            note_path: ranked.candidate.note_path.clone(),
            target: ranked.candidate.target.display().to_string(),
            line: ranked.candidate.line,
            task_ref: ranked.candidate.task_ref.clone(),
            block_id: ranked.candidate.block_id.clone(),
            score: ranked.score,
        })
        .collect();
    CaptureRelatedResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        warnings: discovery.warnings,
        items: vec![RelatedItem {
            index: 0,
            mode: "task".to_string(),
            matches,
        }],
    }
}

fn task_fields_date(date: chrono::NaiveDate) -> String {
    super::task_fields::format_calendar_date(date)
}

fn print_success(result: &CaptureRelatedResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human(result, &Styler::detect()),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(result)
                    .expect("serialize capture-related result")
            );
        }
    }
}

fn print_human(result: &CaptureRelatedResult, styler: &Styler) {
    if result.items.is_empty() {
        println!("{}", styler.dim("No similar tasks."));
        print_warnings(result, styler);
        return;
    }
    let item_count = result.items.len();
    for item in &result.items {
        println!(
            "{}  {}",
            styler.cyan(&format!("{}/{}", item.index + 1, item_count)),
            styler.dim(&item.mode)
        );
        if item.matches.is_empty() {
            println!("  {}", styler.dim("no matches"));
            continue;
        }
        for related in &item.matches {
            let title =
                bold_title(&related.title, &related.matched_ranges, styler);
            let tag = band_tag(related);
            let locator = if related.status_name.is_empty() {
                related.locator.clone()
            } else {
                format!("{} · {}", related.locator, related.status_name)
            };
            println!(
                "  {}  {}   ( {} )   {}   {}",
                styler.cyan(&related.status_symbol),
                title,
                styler.yellow(&tag),
                styler.dim(&locator),
                styler.dim(&score_bar(related.score)),
            );
        }
        println!();
    }
    print_warnings(result, styler);
}

fn print_warnings(result: &CaptureRelatedResult, styler: &Styler) {
    if result.warnings.is_empty() {
        return;
    }
    println!("  {}", styler.dim("Warnings"));
    for warning in &result.warnings {
        println!("    {}", styler.yellow(warning));
    }
}

fn band_tag(related: &RelatedMatch) -> String {
    match related.band.as_str() {
        "same_title" => "Same title".to_string(),
        "possible_duplicate" => "Possible duplicate".to_string(),
        "similar" => "Similar".to_string(),
        "closed_duplicate" => related
            .closed
            .as_ref()
            .map(|closed| {
                let status = if closed.status == "canceled" {
                    "Canceled"
                } else {
                    "Done"
                };
                format!("{status} {}d ago", closed.days_ago)
            })
            .unwrap_or_else(|| "Closed duplicate".to_string()),
        "hold" => "Hold".to_string(),
        other => other.to_string(),
    }
}

fn bold_title(
    title: &str,
    ranges: &[(usize, usize)],
    styler: &Styler,
) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    for &(start, end) in ranges {
        if start < cursor || end > title.len() || start >= end {
            continue;
        }
        if !title.is_char_boundary(start) || !title.is_char_boundary(end) {
            continue;
        }
        out.push_str(&title[cursor..start]);
        out.push_str(&styler.bold(&title[start..end]));
        cursor = end;
    }
    out.push_str(&title[cursor..]);
    out
}

fn score_bar(score: f64) -> String {
    let filled = ((score * 10.0).round() as usize).min(10);
    let bar: String = (0..10)
        .map(|index| if index < filled { '█' } else { '░' })
        .collect();
    format!("{bar}  {score:.3}")
}

fn print_error(message: &str, output_format: OutputFormat, code: i32) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {message}"),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }))
        }
    }
    code
}
