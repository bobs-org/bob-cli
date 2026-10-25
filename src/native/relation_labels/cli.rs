use std::{ffi::OsString, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    env as bob_env,
    relation_labels::LabelSpelling,
    style::{pad_right, Styler},
};

use super::{Catalog, HiddenFilter};

const COMMAND_NAME: &str = "bob relations list";
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_list();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let format = OutputFormat::from_matches(&matches);
    let request = ListRequest::from_matches(&matches);
    match list_labels(&request) {
        Ok(report) => {
            print_success(&report, format);
            0
        }
        Err(error) => print_error(error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli_list() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List relation labels in the vault (default)")
        .long_about(
            "List the relation-label vocabulary derived from every LINKS \
entry in the vault: open, closed, done/, daily notes, nested projects, \
and references. The catalog is read-only: it never writes the vault. \
Bare `bob relations` runs this command.\n\n\
By default the list shows visible labels, most-used first. Hidden \
labels stay out of the default list; they still label their links. \
Use --all to include every known label, --hidden to show only hidden \
labels, --query to rank exact/prefix matches before substring matches.",
        )
        .after_help(
            "Examples:\n  \
bob relations\n  \
bob relations list\n  \
bob relations -q insp -f json\n  \
bob relations -a\n  \
bob relations -H\n  \
bob relations -n 10 -q dup\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
NO_COLOR                  Disable colored output\n\n\
Exit codes:\n  \
0  list\n  \
1  vault I/O failure\n  \
2  usage or config error\n\n\
See docs/task-relations.md for the label-vocabulary contract.",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(hidden_arg())
        .arg(limit_arg())
        .arg(query_arg())
}

fn all_arg() -> Arg {
    Arg::new("all")
        .long("all")
        .short('a')
        .action(ArgAction::SetTrue)
        .help("List every known label, flagging hidden ones")
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

fn hidden_arg() -> Arg {
    Arg::new("hidden")
        .long("hidden")
        .short('H')
        .action(ArgAction::SetTrue)
        .help("List only hidden labels")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('n')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("Show at most N labels after filtering and ranking")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .short('q')
        .value_name("Q")
        .help("Rank exact and prefix matches first, then substring matches")
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

#[derive(Debug, Clone)]
struct ListRequest {
    bob_dir: PathBuf,
    all: bool,
    hidden_only: bool,
    limit: Option<usize>,
    query: Option<String>,
}

impl ListRequest {
    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            bob_dir: matches
                .get_one::<OsString>("bob-dir")
                .map(PathBuf::from)
                .map(|path| bob_env::expand_tilde(&path))
                .unwrap_or_else(bob_env::bob_dir),
            all: matches.get_flag("all"),
            hidden_only: matches.get_flag("hidden"),
            limit: matches.get_one::<usize>("limit").copied(),
            query: matches.get_one::<String>("query").cloned(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ListLabel {
    key: String,
    name: String,
    count: u64,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    inverse: Option<String>,
    spellings: Vec<LabelSpelling>,
    replacement: String,
    hide_replacement: String,
}

#[derive(Debug, Clone, Serialize)]
struct ListCounts {
    labels: usize,
    links: u64,
    hidden: usize,
}

#[derive(Debug, Clone, Serialize)]
struct ListReport {
    schema_version: u32,
    ok: bool,
    counts: ListCounts,
    labels: Vec<ListLabel>,
    renamed_reappeared: Vec<serde_json::Value>,
    warnings: Vec<String>,
}

enum ListError {
    Io(String),
}

fn list_labels(request: &ListRequest) -> Result<ListReport, ListError> {
    let catalog = Catalog::load(&request.bob_dir).map_err(|error| {
        ListError::Io(format!(
            "read relation catalog in {}: {error}",
            request.bob_dir.display()
        ))
    })?;
    Ok(build_report(request, &catalog))
}

fn build_report(request: &ListRequest, catalog: &Catalog) -> ListReport {
    let hidden_filter = if request.hidden_only {
        HiddenFilter::Only
    } else if request.all {
        HiddenFilter::Include
    } else {
        HiddenFilter::Exclude
    };
    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .unwrap_or("");
    let mut selected = catalog.rank_for_query(query, hidden_filter);
    if let Some(limit) = request.limit {
        selected.truncate(limit);
    }
    let labels = selected
        .iter()
        .map(|record| ListLabel {
            key: record.key.clone(),
            name: record.name.clone(),
            count: record.count,
            hidden: catalog.is_hidden(&record.key),
            inverse: record.inverse.clone(),
            spellings: record.spellings.clone(),
            replacement: record.replacement(),
            hide_replacement: record.hide_replacement(),
        })
        .collect::<Vec<_>>();
    ListReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        counts: ListCounts {
            labels: catalog.known_count(),
            links: catalog.links_total(),
            hidden: catalog.hidden_count(),
        },
        labels,
        renamed_reappeared: super::rename::renamed_reappeared(
            &request.bob_dir,
            catalog,
        ),
        warnings: catalog.warnings().to_vec(),
    }
}

fn print_success(report: &ListReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}");
            }
        },
        OutputFormat::Human => print_human(report),
    }
}

fn print_human(report: &ListReport) {
    let styler = Styler::detect();
    let heading = if styler.is_color() {
        styler.paint("38;5;121", "RELATIONS")
    } else {
        "RELATIONS".to_string()
    };
    let counts = format!(
        "{} {} {} {} {} {}",
        report.counts.labels,
        label_word(report.counts.labels),
        styler.separator(),
        format!("{} {}", report.counts.links, link_word(report.counts.links)),
        styler.separator(),
        format!("{} hidden", report.counts.hidden,),
    );
    println!("{heading}  {counts}");
    if report.labels.is_empty() {
        println!("  (no relation labels)");
    } else {
        let name_width = report
            .labels
            .iter()
            .map(|label| label.name.chars().count())
            .max()
            .unwrap_or(8)
            .clamp(8, 24);
        for label in &report.labels {
            let links = format!("{} {}", label.count, link_word(label.count));
            let mut meta = if styler.is_color() {
                styler.dim(&links)
            } else {
                links
            };
            if let Some(inverse) = label.inverse.as_deref() {
                meta.push_str(&format!("     ⇄ {inverse}"));
            }
            if label.hidden {
                meta.push_str("     ◌ hidden");
            }
            println!("  {}  {meta}", pad_right(&label.name, name_width));
        }
    }
    for warning in &report.warnings {
        eprintln!("{COMMAND_NAME}: {warning}");
    }
}

fn label_word(count: usize) -> &'static str {
    if count == 1 {
        "label"
    } else {
        "labels"
    }
}

fn link_word(count: u64) -> &'static str {
    if count == 1 {
        "link"
    } else {
        "links"
    }
}

fn print_error(error: ListError, format: OutputFormat) -> i32 {
    let ListError::Io(message) = error;
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "code": "io",
                    "message": message,
                })
            );
        }
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {message}");
        }
    }
    1
}
