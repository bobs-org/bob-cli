//! CLI builders for the `bob ref` library verbs.
//!
//! `find` looks up batch identity queries; `list` renders filtered
//! library views with the reading queue as its default; `show` resolves
//! exact references into metadata, annotations, notes, and tasks. Shared
//! directory args reuse the Highlights builders so help strings stay
//! identical.
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::find::resolve_find;
use super::output::{
    print_find_error, print_find_json, print_list_json, render_find_human,
    render_find_markdown, render_list_human, render_list_markdown,
    CollapsedLegacy, Format,
};
use super::{
    build_index, fill_git_dates, filter_list, parse_since_cutoff, select,
    validate_since, Coverage, LibraryConfig, ListSelection, RefIndex, RefRow,
    LIST_STATE_ORDER,
};

pub(crate) use super::show::run_show;
use crate::native::env as bob_env;
use crate::native::highlights_ref::{
    bob_dir_arg, collect_intake_records, ref_dir_arg, xlib_dir_arg,
    IntakeRecord, COMMAND_NAME,
};
use crate::native::style::{terminal_width, Styler};

/// The `bob ref find` subcommand builder.
pub(crate) fn find_command() -> ClapCommand {
    ClapCommand::new("find")
        .about(
            "Look up URLs, arXiv IDs, DOIs, paths, or titles in the reference library",
        )
        .arg(
            Arg::new("query")
                .value_name("QUERY")
                .num_args(1..)
                .required(true)
                .help("URL, arXiv ID, DOI, vault path, note stem or id, or title words; `-` reads one query per line from stdin"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json", "markdown"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("include-intake")
                .long("include-intake")
                .short('i')
                .action(ArgAction::SetTrue)
                .help("Also check queued intake PDFs (reads their markers)"),
        )
        .arg(
            Arg::new("min-score")
                .long("min-score")
                .short('m')
                .value_name("SCORE")
                .value_parser(clap::value_parser!(u64).range(1..=100))
                .default_value("60")
                .help("Minimum title-candidate score, 1-100"),
        )
        .arg(ref_dir_arg())
        .arg(xlib_dir_arg())
        .after_help(
            "A title match is only ever a candidate, never proof that a reference is in the library. `not found` means only \"not under ref/\".\n\
            \n\
            Verdicts: in_library (an exact URL, identifier, path, or name match), in_intake (only a queued intake PDF matched, with -i), possible (only title candidates at or above --min-score), not_found (nothing matched).\n\
            \n\
            Examples:\n  \
            bob ref find https://arxiv.org/abs/1706.03762\n  \
            printf '%s\\n' URL1 URL2 | bob ref find - -f json\n  \
            bob ref find 1706.03762 -i",
        )
}

/// The `bob ref list` subcommand builder.
pub(crate) fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about(
            "List reference notes by reading state, status, type, origin, or date",
        )
        .arg(
            Arg::new("all")
                .long("all")
                .short('A')
                .action(ArgAction::SetTrue)
                .conflicts_with("limit")
                .help("Show every matching note instead of the first --limit"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json", "markdown"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("git-dates")
                .long("git-dates")
                .short('g')
                .action(ArgAction::SetTrue)
                .help("Fill missing added/finished dates from vault Git history (slower)"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .short('n')
                .value_name("N")
                .value_parser(clap::value_parser!(u64).range(1..))
                .default_value("50")
                .help("Show at most N notes"),
        )
        .arg(
            Arg::new("origin")
                .long("origin")
                .short('o')
                .value_name("ORIGIN")
                .value_parser(["external", "agent-report"])
                .help("Only external references or agent reports"),
        )
        .arg(
            Arg::new("parent")
                .long("parent")
                .short('P')
                .value_name("NOTE")
                .help("Only notes whose parent is this bare note name"),
        )
        .arg(
            Arg::new("reading-state")
                .long("reading-state")
                .short('R')
                .value_name("STATE")
                .num_args(1..)
                .value_delimiter(',')
                .value_parser([
                    "queued",
                    "started",
                    "finished",
                    "dropped",
                    "unknown",
                    "all",
                ])
                .help("queued, started, finished, dropped, unknown, or all (comma-separated)"),
        )
        .arg(ref_dir_arg())
        .arg(
            Arg::new("since")
                .long("since")
                .short('S')
                .value_name("DATE")
                .value_parser(validate_since)
                .help("Only notes whose row date is on or after DATE (YYYY-MM-DD, 7d, 4w, 6m, 1y)"),
        )
        .arg(
            Arg::new("status")
                .long("status")
                .short('s')
                .value_name("STATUS")
                .num_args(1..)
                .value_delimiter(',')
                .value_parser([
                    "ready",
                    "next",
                    "wip",
                    "read",
                    "abandoned",
                    "legacy",
                    "conflict",
                    "unknown",
                ])
                .help("ready, next, wip, read, abandoned, legacy, conflict, unknown (comma-separated)"),
        )
        .arg(
            Arg::new("ref-type")
                .long("ref-type")
                .short('t')
                .value_name("TYPE")
                .num_args(1..)
                .value_delimiter(',')
                .help("Library subdirectory, such as papers, blogs, docs, chat, or ai (comma-separated)"),
        )
        .after_help(
            "With no filter option at all, list shows the reading queue (queued and started notes) in every format. Any filter option searches every reading state unless -R narrows it.\n\
            \n\
            Examples:\n  \
            bob ref list\n  \
            bob ref list -R finished -S 30d -g\n  \
            bob ref list -o external -R finished -f json\n  \
            bob ref list -s legacy -R queued",
        )
}

/// The `bob ref show` subcommand builder.
pub(crate) fn show_command() -> ClapCommand {
    ClapCommand::new("show")
        .about(
            "Show reference notes with metadata, annotations, and your own notes",
        )
        .arg(
            Arg::new("ref")
                .value_name("REF")
                .num_args(1..)
                .required(true)
                .help("Vault path, note stem or id, URL, arXiv ID, DOI, or exact title"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("comments-only")
                .long("comments-only")
                .short('c')
                .action(ArgAction::SetTrue)
                .conflicts_with("no-annotations")
                .help("Only annotations you commented on and standalone notes, with their quotes"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json", "markdown"])
                .default_value("human")
                .help("Output format"),
        )
        .arg(
            Arg::new("no-annotations")
                .long("no-annotations")
                .short('N')
                .action(ArgAction::SetTrue)
                .help("Metadata and your own notes only"),
        )
        .arg(ref_dir_arg())
        .after_help(
            "Resolution is exact (a vault path, note stem or id, URL, arXiv ID, DOI, or exact title). Several matches collapse to the one note that is not superseded; any other multi-match is ambiguous and names its candidates, and a miss suggests up to three title candidates.\n\
            \n\
            Examples:\n  \
            bob ref show ea_graph\n  \
            bob ref show ea_graph -c\n  \
            bob ref show ref/papers/ea_graph.md -f json",
        )
}

/// Run `bob ref list`: build the index, filter and order it, and render.
pub(crate) fn run_list(matches: &ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let config = list_config_from_matches(matches);
    let mut index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref list",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let selection = match list_selection(matches) {
        Ok(selection) => selection,
        Err(message) => {
            print_find_error(
                format,
                "ref list",
                "invalid_option",
                &message,
                None,
            );
            return 2;
        }
    };
    let mut coverage = index.coverage.clone();
    if matches.get_flag("git-dates") {
        let (git_word, warning) = fill_git_dates(&config, &mut index.rows);
        coverage.git_dates = git_word;
        if let Some(warning) = warning {
            eprintln!("{COMMAND_NAME}: warning: {warning}");
        }
    }
    let outcome = filter_list(&index.rows, &selection);
    let matched = outcome.ordered.len();
    // Without `-s`, legacy-era rows collapse to one human summary line
    // instead of listing; JSON and Markdown still carry every match.
    let collapse = !selection.has_status_filter();
    let listed: Vec<usize> = outcome
        .ordered
        .iter()
        .copied()
        .filter(|row| !collapse || index.rows[*row].era != "legacy")
        .collect();
    let listed_total = listed.len();
    let limit: usize = selection
        .limit
        .and_then(|capped| usize::try_from(capped).ok())
        .unwrap_or(usize::MAX);
    let shown: Vec<usize> = listed.iter().copied().take(limit).collect();
    let truncated_more = listed_total.saturating_sub(shown.len());
    let refs: Vec<RefRow> =
        shown.iter().map(|row| index.rows[*row].clone()).collect();
    match format {
        Format::Json => {
            let json_refs: Vec<RefRow> = outcome
                .ordered
                .iter()
                .take(limit)
                .map(|row| index.rows[*row].clone())
                .collect();
            print_list_json(
                &coverage,
                &selection,
                matched,
                json_refs.len(),
                matched > json_refs.len(),
                outcome.hidden_superseded,
                outcome.undated_excluded,
                &index.counts,
                &json_refs,
            );
        }
        Format::Markdown => {
            let markdown_refs: Vec<RefRow> = outcome
                .ordered
                .iter()
                .take(limit)
                .map(|row| index.rows[*row].clone())
                .collect();
            print!(
                "{}",
                render_list_markdown(&coverage, matched, &markdown_refs)
            );
        }
        Format::Human => {
            print!(
                "{}",
                render_list_human(
                    &selection,
                    &coverage,
                    &index.counts,
                    listed_total,
                    &refs,
                    &group_totals(&index.rows, &listed),
                    truncated_more,
                    &collapse_legacy(&index.rows, &outcome.ordered, collapse),
                    Styler::detect(),
                    terminal_width(),
                )
            );
        }
    }
    0
}

/// The effective selection from the `list` matches. `--since` values are
/// already shape-checked by clap; resolve the cutoff against the clock.
fn list_selection(matches: &ArgMatches) -> Result<ListSelection, String> {
    let states = matches
        .get_many::<String>("reading-state")
        .map(|values| values.cloned().collect::<Vec<_>>());
    let statuses = matches
        .get_many::<String>("status")
        .map(|values| values.cloned().collect::<Vec<_>>());
    let ref_types = matches
        .get_many::<String>("ref-type")
        .map(|values| values.cloned().collect::<Vec<_>>());
    let origin = matches.get_one::<String>("origin").cloned();
    let parent = matches.get_one::<String>("parent").cloned();
    let since = matches.get_one::<String>("since").cloned();
    let since_cutoff = since
        .as_deref()
        .map(|value| {
            parse_since_cutoff(value, &bob_env::current_datetime().date())
                .ok_or_else(|| format!("invalid --since {value:?}"))
        })
        .transpose()?;
    let limit = if matches.get_flag("all") {
        None
    } else {
        Some(matches.get_one::<u64>("limit").copied().unwrap_or(50))
    };
    Ok(select(
        states,
        statuses,
        ref_types,
        origin,
        parent,
        since,
        since_cutoff,
        limit,
    ))
}

/// Pre-cap per-state totals over the listed rows, in state order, for
/// the human group headings.
fn group_totals(
    rows: &[RefRow],
    listed: &[usize],
) -> Vec<(&'static str, usize)> {
    LIST_STATE_ORDER
        .iter()
        .map(|state| {
            let count = listed
                .iter()
                .filter(|row| rows[**row].reading_state == *state)
                .count();
            (*state, count)
        })
        .filter(|(_, count)| *count > 0)
        .collect()
}

/// The legacy-era summary for the human collapse line, in state order.
/// Empty unless collapsing is active and legacy rows matched.
fn collapse_legacy(
    rows: &[RefRow],
    ordered: &[usize],
    collapse: bool,
) -> Vec<CollapsedLegacy> {
    if !collapse {
        return Vec::new();
    }
    LIST_STATE_ORDER
        .iter()
        .filter_map(|state| {
            let count = ordered
                .iter()
                .filter(|index| {
                    rows[**index].era == "legacy"
                        && rows[**index].reading_state == *state
                })
                .count();
            (count > 0).then_some(CollapsedLegacy { state, count })
        })
        .collect()
}

/// Library locations from the `list` matches: only `bob-dir` and
/// `ref-dir` are read, never the full Highlights config (whose `lib-dir`
/// id this command does not define) and never `xlib-dir`.
pub(crate) fn list_config_from_matches(matches: &ArgMatches) -> LibraryConfig {
    let bob_dir = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let ref_dir = library_dir(
        matches,
        "ref-dir",
        "BOB_HIGHLIGHTS_REF_DIR",
        "ref",
        &bob_dir,
    );
    LibraryConfig {
        xlib_dir: bob_dir.join("xlib"),
        bob_dir,
        ref_dir,
    }
}

/// Library locations from the `find` matches: only `bob-dir`, `ref-dir`,
/// and `xlib-dir` are read, never the full Highlights config (whose
/// `lib-dir` id this command does not define).
pub(crate) fn library_config_from_matches(
    matches: &ArgMatches,
) -> LibraryConfig {
    let bob_dir = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let ref_dir = library_dir(
        matches,
        "ref-dir",
        "BOB_HIGHLIGHTS_REF_DIR",
        "ref",
        &bob_dir,
    );
    let xlib_dir = library_dir(
        matches,
        "xlib-dir",
        "BOB_HIGHLIGHTS_XLIB_DIR",
        "xlib",
        &bob_dir,
    );
    LibraryConfig {
        bob_dir,
        ref_dir,
        xlib_dir,
    }
}

fn library_dir(
    matches: &ArgMatches,
    arg_name: &str,
    env_name: &str,
    default_value: &str,
    bob_dir: &std::path::Path,
) -> PathBuf {
    let configured = matches
        .get_one::<OsString>(arg_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(env_name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(default_value));
    let expanded = bob_env::expand_tilde(&configured);
    if expanded.is_absolute() {
        expanded
    } else {
        bob_dir.join(expanded)
    }
}

/// Run `bob ref find`: build the index, resolve every query, and render.
pub(crate) fn run_find(matches: &ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let config = library_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref find",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let queries = match collect_queries(matches) {
        Ok(queries) => queries,
        Err(message) => {
            print_find_error(format, "ref find", "read_failed", &message, None);
            return 1;
        }
    };
    let include_intake = matches.get_flag("include-intake");
    let (intake, intake_word) = collect_intake(&config, include_intake);
    let intake = match intake {
        Ok(intake) => intake,
        Err(message) => {
            print_find_error(format, "ref find", "read_failed", &message, None);
            return 1;
        }
    };
    let min_score = matches.get_one::<u64>("min-score").copied().unwrap_or(60);
    let (results, summary) =
        resolve_find(&index, &queries, min_score, intake.as_deref());
    let coverage = coverage_with_intake(&index, intake_word);
    match format {
        Format::Json => {
            print_find_json(&coverage, &index.counts, &summary, &results);
        }
        Format::Markdown => {
            print!("{}", render_find_markdown(&summary, &results));
        }
        Format::Human => {
            print!(
                "{}",
                render_find_human(
                    &coverage,
                    &summary,
                    &results,
                    intake_word,
                    Styler::detect(),
                    terminal_width(),
                )
            );
        }
    }
    0
}

/// Queries from argv, in order with duplicates kept; each `-` splices in
/// one query per stdin line.
fn collect_queries(matches: &ArgMatches) -> Result<Vec<String>, String> {
    let raw = matches
        .get_many::<String>("query")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if !raw.iter().any(|query| query == "-") {
        return Ok(raw);
    }
    let stdin = std::io::read_to_string(std::io::stdin())
        .map_err(|error| format!("read stdin: {error}"))?;
    let lines = stdin
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut queries = Vec::new();
    for query in raw {
        if query == "-" {
            queries.extend(lines.iter().cloned());
        } else {
            queries.push(query);
        }
    }
    Ok(queries)
}

/// Queued intake records for `-i`, plus the coverage word for the
/// envelope and the human footer.
fn collect_intake(
    config: &LibraryConfig,
    include_intake: bool,
) -> (Result<Option<Vec<IntakeRecord>>, String>, &'static str) {
    if !include_intake {
        return (Ok(None), "intake not checked");
    }
    // A missing intake dir is coverage, not failure: the envelope and
    // the human footer carry `unavailable` and stay silent otherwise.
    match collect_intake_records(&config.xlib_dir) {
        Some(Ok(records)) => (Ok(Some(records)), "intake checked"),
        Some(Err(error)) => (Err(error.to_string()), "intake unavailable"),
        None => (Ok(None), "intake unavailable"),
    }
}

/// Coverage with the intake state for this run.
pub(crate) fn coverage_with_intake(
    index: &RefIndex,
    intake_word: &str,
) -> Coverage {
    let mut coverage = index.coverage.clone();
    coverage.intake = match intake_word {
        "intake checked" => "checked".to_string(),
        "intake unavailable" => "unavailable".to_string(),
        _ => "not_checked".to_string(),
    };
    coverage
}
