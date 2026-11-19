use std::{ffi::OsString, path::PathBuf, time::SystemTime};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use crate::native::{env as bob_env, style::Styler};

use super::{
    aggregate_excluded, build_report, parse_kind, rank, LogCatalog, LogKind,
    SCHEMA_VERSION,
};

const COMMAND_NAME: &str = "bob logs list";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME))
            .chain(args.iter().cloned()),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            if asks_for_json(&args) {
                print_json_error("usage", &error.to_string());
            } else if let Err(print_error) = error.print()
                && print_error.kind() != std::io::ErrorKind::BrokenPipe
            {
                eprintln!("{COMMAND_NAME}: failed to print command-line error: {print_error}");
            }
            return error.exit_code();
        }
    };
    let format = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human");
    let kinds = selected_kinds(&matches);
    let query = matches.get_one::<String>("query").map(String::as_str);
    let bob_dir = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let catalog = match super::load(&bob_dir, None) {
        Ok(catalog) => catalog,
        Err(error) => {
            if format == "json" {
                print_json_error("vault_unavailable", &error.to_string());
            } else {
                eprintln!("{COMMAND_NAME}: {error}");
            }
            return 1;
        }
    };
    let default_limit = if format == "json" { 200 } else { 10 };
    let limit = matches
        .get_one::<usize>("limit")
        .copied()
        .unwrap_or(default_limit);
    let report = build_report(&catalog, &kinds, query, limit);
    if format == "json" {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                print_json_error("serialization", &error.to_string());
                return 1;
            }
        }
    } else {
        print_human(&catalog, &kinds, query, limit);
    }
    0
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List reusable phrases from task logs (default)")
        .long_about(
            "List human-written summaries from the vault's Schedule, Work, Rotten, and Cancel Logs. The catalog is derived on each call and never writes to the vault. Matching uses exact, prefix, word-prefix, then substring tiers; the newest match is pinned before frecency ranking.\n\nBare `bob logs` runs this command. Human output shows 10 phrases per log by default; JSON shows 200.",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(limit_arg())
        .arg(log_arg())
        .arg(query_arg())
        .after_help(
            "Examples:\n  \\
             bob logs\n  \\
             bob logs list -l work\n  \\
             bob logs -l work -q 'athena' -n 25\n  \\
             bob logs -l schedule -f json\n  \\
             bob logs -l work -l cancel -b ~/bob -f json\n\nEnvironment:\n  \\
             BOB_DIR          Bob vault root when --bob-dir is omitted\n  \\
             NO_COLOR         Disable colored output\n\nExit codes:\n  \\
             0  list\n  \\
             1  vault I/O failure\n  \\
             2  usage or config error\n\nSee docs/log-recall.md for the log-history contract.",
        )
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

fn log_arg() -> Arg {
    Arg::new("log")
        .long("log")
        .short('l')
        .value_name("KIND")
        .value_parser(["schedule", "work", "rotten", "cancel"])
        .action(ArgAction::Append)
        .help("Show one log kind; repeat to select several (default: all four)")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('n')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("Show at most N phrases per log (human default 10; JSON default 200)")
}

fn query_arg() -> Arg {
    Arg::new("query")
        .long("query")
        .short('q')
        .value_name("TEXT")
        .help("Filter and rank by an exact, prefix, word-prefix, or substring match")
}

fn selected_kinds(matches: &ArgMatches) -> Vec<LogKind> {
    let requested = matches
        .get_many::<String>("log")
        .map(|values| values.cloned().collect::<Vec<_>>());
    match requested {
        None => LogKind::ALL.to_vec(),
        Some(values) => LogKind::ALL
            .into_iter()
            .filter(|kind| {
                values.iter().any(|value| parse_kind(value) == Some(*kind))
            })
            .collect(),
    }
}

fn asks_for_json(args: &[OsString]) -> bool {
    args.iter()
        .any(|arg| arg == "--format=json" || arg == "-fjson")
        || args.windows(2).any(|pair| {
            pair[0] == "--format" && pair[1] == "json"
                || pair[0] == "-f" && pair[1] == "json"
        })
}

fn print_json_error(code: &str, message: &str) {
    println!(
        "{}",
        json!({
            "schema_version": SCHEMA_VERSION,
            "ok": false,
            "error": { "code": code, "message": message },
        })
    );
}

fn print_human(
    catalog: &LogCatalog,
    kinds: &[LogKind],
    query: Option<&str>,
    limit: usize,
) {
    let styler = Styler::detect();
    for (index, kind) in kinds.iter().copied().enumerate() {
        if index > 0 {
            println!();
        }
        let ranked = rank::rank(catalog.phrases_for(kind), query);
        let precision = if kind == LogKind::Schedule {
            " · dates approximate"
        } else {
            ""
        };
        println!(
            "{} {} · {} phrases{}",
            kind.emoji(),
            styler.bold(kind.label()),
            catalog.phrases_for(kind).len(),
            precision
        );
        for row in ranked.iter().take(limit) {
            let age =
                format_age(&row.phrase.last_used, kind, SystemTime::now());
            let phrase = highlight(&row.phrase.text, query, &styler);
            println!("  {:>4}×  {:<10}  {}", row.phrase.uses, age, phrase);
        }
        if ranked.len() > limit {
            println!(
                "  {}",
                styler.dim(&format!(
                    "… {} more · bob logs -l {} -n {}",
                    ranked.len() - limit,
                    kind.as_str(),
                    limit.max(10).saturating_add(40)
                ))
            );
        }
    }
    let excluded = aggregate_excluded(catalog, kinds);
    if excluded.machine
        + excluded.copies
        + excluded.too_long
        + excluded.blank
        + excluded.unparsed
        > 0
    {
        println!("\n{}", styler.dim(&format!(
            "excluded {} machine entries · {} split/merge copies · {} over 200 chars · {} blank · {} unparsed",
            excluded.machine, excluded.copies, excluded.too_long, excluded.blank, excluded.unparsed
        )));
    }
    for warning in &catalog.warnings {
        eprintln!("{}", styler.yellow(&format!("warning: {warning}")));
    }
}

fn format_age(last_used: &str, kind: LogKind, now: SystemTime) -> String {
    let now: chrono::DateTime<chrono::Local> = now.into();
    let date = chrono::NaiveDate::parse_from_str(
        &last_used[..10.min(last_used.len())],
        "%Y-%m-%d",
    )
    .ok();
    let Some(date) = date else {
        return "unknown".into();
    };
    let days = now.date_naive().signed_duration_since(date).num_days();
    if kind == LogKind::Work
        && last_used.len() > 10
        && let Ok(used) =
            chrono::NaiveDateTime::parse_from_str(last_used, "%Y-%m-%d %H:%M")
    {
        let hours = now
            .naive_local()
            .signed_duration_since(used)
            .num_hours()
            .max(0);
        if hours < 1 {
            return "now".into();
        }
        if hours < 24 {
            return format!("{hours}h ago");
        }
    }
    match days {
        i64::MIN..=0 => "today".into(),
        1 => "yesterday".into(),
        value => format!("{value}d ago"),
    }
}

fn highlight(text: &str, query: Option<&str>, styler: &Styler) -> String {
    let Some(query) = query
        .map(super::rank::normalize)
        .filter(|query| !query.is_empty())
    else {
        return text.into();
    };
    let lowered = text.to_lowercase();
    let Some((start, found)) = lowered.match_indices(&query).next() else {
        return text.into();
    };
    if lowered.chars().count() != text.chars().count() {
        return styler.cyan(text);
    }
    let start_char = lowered[..start].chars().count();
    let end_char = start_char + found.chars().count();
    let chars = text.chars().collect::<Vec<_>>();
    let before = chars[..start_char].iter().collect::<String>();
    let matched = chars[start_char..end_char].iter().collect::<String>();
    let after = chars[end_char..].iter().collect::<String>();
    format!("{before}{}{after}", styler.cyan(&matched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_kind_selection_preserves_canonical_order_and_repeated_kinds() {
        let command = build_cli();
        let matches = command
            .try_get_matches_from(["logs", "-l", "cancel", "-l", "work"])
            .unwrap();
        assert_eq!(selected_kinds(&matches), [LogKind::Work, LogKind::Cancel]);
    }

    #[test]
    fn json_usage_detection_covers_short_and_long_forms() {
        assert!(asks_for_json(&["-f".into(), "json".into()]));
        assert!(asks_for_json(&["--format=json".into()]));
        assert!(!asks_for_json(&["-f".into(), "human".into()]));
    }
}
