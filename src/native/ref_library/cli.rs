//! CLI builders for the `bob ref` library verbs.
//!
//! This phase registers only `find`; `list` and `show` add their builders
//! here in their own phases. Shared directory args reuse the Highlights
//! builders so help strings stay identical.
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::find::resolve_find;
use super::output::{
    print_find_error, print_find_json, render_find_human, render_find_markdown,
    Format,
};
use super::{build_index, Coverage, LibraryConfig, RefIndex};
use crate::native::env as bob_env;
use crate::native::highlights_ref::{
    bob_dir_arg, collect_intake_records, ref_dir_arg, xlib_dir_arg,
    IntakeRecord,
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
