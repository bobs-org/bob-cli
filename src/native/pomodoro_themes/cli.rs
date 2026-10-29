use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};

use chrono::NaiveDate;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    env as bob_env,
    pomodoro_homes::Hue,
    style::{pad_right, Styler},
};

use super::{hints, Catalog, HueSource, ThemeRecord, TodayPresence};

const COMMAND_NAME: &str = "bob types list";
const SCHEMA_VERSION: u32 = 1;
const DEFAULT_WINDOW_DAYS: i64 = 90;

pub(crate) fn run_list(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let format = OutputFormat::from_matches(&matches);
    let request = ListRequest::from_matches(&matches);
    match list_themes(&request) {
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

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List remembered task types (default)")
        .long_about(
            "List task types derived from canonical daily notes.\n\n\
A task type names the kind of work a task represents; a Pomodoro name may contain several type components. The catalog is read-only: \
it never writes the vault. Bare `bob types` runs this command.\n\n\
By default the list shows visible types used in the last 90 days or \
today, newest first. Hidden types stay out of the default list; they \
still exist in history, Stats, and colors. Use --all to include every \
known type, --hidden to show only hidden types, --query to rank \
exact/prefix matches before substring matches, and --fresh to ignore \
the machine-local cache. When a `themes:` vocabulary is configured in \
the config file, the default list also shows every visible configured \
type, even ones never used.",
        )
        .after_help(
            "Examples:\n  \
bob types\n  \
bob types list\n  \
bob types -q fix -f json\n  \
bob types -a -F\n  \
bob types -H\n  \
bob types -n 10 -q bo\n\n\
Environment:\n  \
BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  \
BOB_DIR                   Bob vault root when --bob-dir is omitted\n  \
BOB_NOW                   Local datetime override that pins today\n  \
NO_COLOR                  Disable colored output\n  \
XDG_CACHE_HOME            Cache root; catalog lives in <cache>/bob-cli/themes/\n\n\
Exit codes:\n  \
0  list\n  \
1  vault I/O failure\n  \
2  usage or config error\n\n\
See docs/task-types.md for identity, cache, hue, JSON, and journal writers.",
        )
        .disable_help_flag(true)
        .arg(all_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(fresh_arg())
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
        .help("List every known type, flagging hidden ones")
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

fn fresh_arg() -> Arg {
    Arg::new("fresh")
        .long("fresh")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Rebuild the catalog and note-default facts, ignoring caches")
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
        .help("List only hidden types")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('n')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("Show at most N types after filtering and ranking")
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
    fresh: bool,
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
            fresh: matches.get_flag("fresh"),
            hidden_only: matches.get_flag("hidden"),
            limit: matches.get_one::<usize>("limit").copied(),
            query: matches.get_one::<String>("query").cloned(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ListTheme {
    key: String,
    name: String,
    hue: Option<Hue>,
    hue_source: HueSource,
    pinned: bool,
    hidden: bool,
    exempt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<bool>,
    first_used: Option<String>,
    last_used: Option<String>,
    days_used: u32,
    entries: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    today: Option<TodayPresence>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfiguredThemeJson {
    key: String,
    name: String,
    color: Option<Hue>,
}

#[derive(Debug, Clone, Serialize)]
struct ListReport {
    schema_version: u32,
    today: String,
    window_days: u32,
    counts: ListCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured_themes: Option<Vec<ConfiguredThemeJson>>,
    themes: Vec<ListTheme>,
    hints: Vec<hints::ThemeHint>,
    renamed_reappeared: Vec<serde_json::Value>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ListCounts {
    #[serde(skip_serializing_if = "Option::is_none")]
    configured: Option<usize>,
    known: usize,
    listed: usize,
    today: usize,
    hidden: usize,
}

enum ListError {
    Io(String),
}

fn list_themes(request: &ListRequest) -> Result<ListReport, ListError> {
    let today = bob_env::current_datetime().date();
    // The independent caches can be refreshed concurrently. Capture the cache
    // location on this thread so scoped environment overrides remain effective.
    let cache_dir = bob_env::bob_cli_cache_dir();
    let (defaults, catalog) = std::thread::scope(|scope| {
        let defaults = scope.spawn(|| {
            super::defaults::load(&request.bob_dir, request.fresh, &cache_dir)
        });
        let catalog =
            Catalog::load_without_areas(&request.bob_dir, today, request.fresh);
        (
            defaults.join().expect("note defaults worker panicked"),
            catalog,
        )
    });
    let mut defaults = defaults.map_err(|error| {
        ListError::Io(format!("read note defaults: {error}"))
    })?;
    let mut catalog = catalog.map_err(|error| {
        ListError::Io(format!(
            "read task type catalog in {}: {error}",
            request.bob_dir.display()
        ))
    })?;
    catalog.areas = std::mem::take(&mut defaults.areas);
    let mut report = build_report(request, &catalog);
    let defaults = defaults.unknown(&catalog);
    report
        .hints
        .extend(defaults.into_iter().map(hints::ThemeHint::UnknownDefault));
    Ok(report)
}

fn build_report(request: &ListRequest, catalog: &Catalog) -> ListReport {
    let today = catalog.today;
    let window_start = today
        .checked_sub_signed(chrono::Duration::days(DEFAULT_WINDOW_DAYS - 1))
        .unwrap_or(NaiveDate::MIN);
    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());

    let mut selected: Vec<&ThemeRecord> = if let Some(query) = query {
        catalog
            .rank_for_query(query, &BTreeSet::new())
            .into_iter()
            .filter(|record| {
                include_record(request, catalog, record, window_start, today)
            })
            .collect()
    } else {
        let mut records: Vec<&ThemeRecord> = catalog
            .records()
            .filter(|record| {
                include_record(request, catalog, record, window_start, today)
            })
            .collect();
        records.sort_by(|left, right| {
            right
                .last_used
                .cmp(&left.last_used)
                .then_with(|| right.days_used.cmp(&left.days_used))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.key.cmp(&right.key))
        });
        records
    };

    if let Some(limit) = request.limit {
        selected.truncate(limit);
    }

    let mut themes = selected
        .iter()
        .map(|record| list_theme(catalog, record))
        .collect::<Vec<_>>();
    themes.extend(never_used_configured(catalog, request, &selected, query));
    let configured_themes = catalog.configured_themes().map(|entries| {
        entries
            .iter()
            .map(|entry| ConfiguredThemeJson {
                key: entry.key.clone(),
                name: entry.name.clone(),
                color: entry.color,
            })
            .collect::<Vec<_>>()
    });
    ListReport {
        schema_version: SCHEMA_VERSION,
        today: today.to_string(),
        window_days: u32::try_from(DEFAULT_WINDOW_DAYS).unwrap_or(90),
        counts: ListCounts {
            configured: catalog
                .configured_themes()
                .map(|entries| entries.len()),
            known: catalog.known_count(),
            listed: themes.len(),
            today: catalog.today_count(),
            hidden: catalog.hidden_count(),
        },
        configured_themes,
        themes,
        hints: {
            let mut hints: Vec<hints::ThemeHint> = catalog
                .collision_hints()
                .into_iter()
                .map(hints::ThemeHint::Collision)
                .collect();
            hints.extend(
                catalog
                    .not_configured_hints()
                    .into_iter()
                    .map(hints::ThemeHint::NotConfigured),
            );
            hints
        },
        renamed_reappeared: super::renamed_reappeared(
            &request.bob_dir,
            catalog,
        ),
        warnings: catalog.warnings().to_vec(),
    }
}

/// Never-used configured themes, in config order, sorted last. They show
/// `never` and `0 days` so the whole vocabulary is visible.
fn never_used_configured(
    catalog: &Catalog,
    request: &ListRequest,
    selected: &[&ThemeRecord],
    query: Option<&str>,
) -> Vec<ListTheme> {
    let Some(entries) = catalog.configured_themes() else {
        return Vec::new();
    };
    let selected_keys: BTreeSet<&str> =
        selected.iter().map(|record| record.key.as_str()).collect();
    let query_key = query.map(crate::native::plan_budget::normalize_component);
    entries
        .iter()
        .filter(|entry| !selected_keys.contains(entry.key.as_str()))
        .filter(|entry| {
            let hidden = catalog.is_hidden(&entry.key);
            if request.hidden_only {
                return hidden;
            }
            if !request.all && hidden {
                return false;
            }
            true
        })
        .filter(|entry| {
            let Some(query_key) = query_key.as_deref() else {
                return true;
            };
            let name_key =
                crate::native::plan_budget::normalize_component(&entry.name);
            name_key.contains(query_key) || query_key.contains(&entry.key)
        })
        .map(|entry| {
            let theme_ref = catalog.theme_ref(&entry.key, &entry.name);
            ListTheme {
                key: entry.key.clone(),
                name: entry.name.clone(),
                hue: theme_ref.hue,
                hue_source: theme_ref.hue_source,
                pinned: catalog.pin(&entry.key).is_some(),
                hidden: catalog.is_hidden(&entry.key),
                exempt: catalog.is_exempt(&entry.key),
                configured: theme_ref.configured,
                first_used: None,
                last_used: None,
                days_used: 0,
                entries: 0,
                today: None,
            }
        })
        .collect()
}

fn include_record(
    request: &ListRequest,
    catalog: &Catalog,
    record: &ThemeRecord,
    window_start: NaiveDate,
    today: NaiveDate,
) -> bool {
    let hidden = catalog.is_hidden(&record.key);
    if request.hidden_only {
        return hidden;
    }
    if !request.all && hidden {
        return false;
    }
    if request.all {
        return true;
    }
    record.last_used >= window_start
        || record.today.is_some()
        || record.last_used == today
}

fn list_theme(catalog: &Catalog, record: &ThemeRecord) -> ListTheme {
    let theme_ref = catalog.theme_ref(&record.key, &record.name);
    ListTheme {
        key: record.key.clone(),
        name: record.name.clone(),
        hue: theme_ref.hue,
        hue_source: theme_ref.hue_source,
        pinned: catalog.pin(&record.key).is_some(),
        hidden: catalog.is_hidden(&record.key),
        exempt: catalog.is_exempt(&record.key),
        configured: theme_ref.configured,
        first_used: Some(record.first_used.to_string()),
        last_used: Some(record.last_used.to_string()),
        days_used: record.days_used,
        entries: record.entries,
        today: record.today,
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
    let window = format!("last {} days", report.window_days);
    if let Some(configured) = report.counts.configured {
        println!(
            "TYPES   {configured} configured {} {} known {} {} today {} {} hidden {} {window}",
            styler.separator(),
            report.counts.known,
            styler.separator(),
            report.counts.today,
            styler.separator(),
            report.counts.hidden,
            styler.separator(),
        );
    } else {
        println!(
            "TYPES   {} known {} {} today {} {} hidden {} {window}",
            report.counts.known,
            styler.separator(),
            report.counts.today,
            styler.separator(),
            report.counts.hidden,
            styler.separator(),
        );
    }
    if report.themes.is_empty() {
        println!("  (no types)");
    } else {
        let name_width = report
            .themes
            .iter()
            .map(|theme| theme.name.chars().count())
            .max()
            .unwrap_or(8)
            .clamp(8, 20);
        for theme in &report.themes {
            let swatch = swatch(&styler, theme);
            let when = human_when(theme.last_used.as_deref(), &report.today);
            let days = format!("{} days", theme.days_used);
            let meta = human_meta(theme);
            println!(
                "  {swatch} {}  {}  {}  {meta}",
                pad_right(&theme.name, name_width),
                pad_right(&when, 8),
                pad_right(&days, 10),
            );
        }
    }
    let mut not_configured = 0usize;
    let mut extra_not_configured = 0usize;
    for hint in &report.hints {
        match hint {
            hints::ThemeHint::UnknownDefault(hint) => {
                println!("  hint  {}", hints::human_unknown_default_hint(hint));
            }
            hints::ThemeHint::Collision(hint) => {
                println!("  hint  {}", hints::human_hint(hint));
            }
            hints::ThemeHint::NotConfigured(hint) => {
                if not_configured < hints::NOT_CONFIGURED_HINT_HUMAN_CAP {
                    println!(
                        "  hint  {}",
                        hints::human_not_configured_hint(hint)
                    );
                    not_configured += 1;
                } else {
                    extra_not_configured += 1;
                }
            }
        }
    }
    if extra_not_configured > 0 {
        println!("  hint  … and {extra_not_configured} more · bob types -a");
    }
    for warning in &report.warnings {
        eprintln!("{}: {}", COMMAND_NAME, styler.yellow(warning));
    }
}

fn swatch(styler: &Styler, theme: &ListTheme) -> String {
    if theme.hue_source == HueSource::Unconfigured {
        return if styler.is_color() {
            styler.dim("◌")
        } else {
            "◌".to_string()
        };
    }
    if theme.hue_source == HueSource::Exempt || theme.hue.is_none() {
        return if styler.is_color() {
            styler.dim("□")
        } else {
            "□".to_string()
        };
    }
    let hue = theme.hue.unwrap_or(Hue::Magenta);
    if styler.is_color() {
        styler.paint(&format!("38;5;{}", hue.xterm()), "■")
    } else {
        "■".to_string()
    }
}

fn human_when(last_used: Option<&str>, today: &str) -> String {
    let Some(last_used) = last_used else {
        return "never".to_string();
    };
    if last_used == today {
        return "today".to_string();
    }
    let Ok(date) = NaiveDate::parse_from_str(last_used, "%Y-%m-%d") else {
        return last_used.to_string();
    };
    date.format("%b %-d").to_string()
}

fn human_meta(theme: &ListTheme) -> String {
    let mut parts = Vec::new();
    match theme.hue_source {
        HueSource::Exempt | HueSource::Unconfigured => {
            parts.push("muted".to_string());
        }
        _ => {
            if let Some(hue) = theme.hue {
                parts.push(hue.name().to_string());
            }
        }
    }
    match theme.hue_source {
        HueSource::Config => parts.push("config".to_string()),
        HueSource::Pin => parts.push("pinned".to_string()),
        HueSource::Area => parts.push("area".to_string()),
        HueSource::Exempt => parts.push("exempt".to_string()),
        HueSource::Unconfigured => {
            parts.push("not one of your types".to_string());
        }
        HueSource::Hash => {}
    }
    if theme.hidden {
        parts.push("hidden".to_string());
    }
    if theme.configured == Some(false)
        && theme.hue_source != HueSource::Unconfigured
    {
        parts.push("not one of your types".to_string());
    }
    parts.join(" · ")
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
