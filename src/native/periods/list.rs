//! Basic `bob periods list`: the period notes that exist with
//! their review states and Headlines.
//!
//! The contract lives in `docs/periods.md` §7. Rows print newest first,
//! grouped year → quarter → weeks.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::super::{env as bob_env, style::Styler};
use super::calendar::PeriodId;
use super::note;

const COMMAND_NAME: &str = "bob periods list";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &clap::ArgMatches) -> Self {
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
struct ListRow {
    id: PeriodId,
    kind: &'static str,
    path: String,
    review_state: &'static str,
    headline: Option<String>,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = super::cli::list_command();
    let mut command = command;
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    let problems_only = matches.get_flag("problems");
    match execute(&matches) {
        Ok((rows, problems)) => {
            print_success(&rows, problems.as_deref(), problems_only, format);
            0
        }
        Err(error) => print_error(error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

#[derive(Debug)]
struct ListError {
    message: String,
}

fn execute(
    matches: &clap::ArgMatches,
) -> Result<(Vec<ListRow>, Option<Vec<super::problems::Problem>>), ListError> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(bob_env::bob_dir);
    let kind_filter = matches.get_one::<String>("kind").cloned();
    let year_filter = matches.get_one::<String>("year").cloned();
    let year_filter: Option<i32> = match year_filter {
        Some(raw) => match raw.trim().parse::<i32>() {
            Ok(year) if (1970..=2999).contains(&year) => Some(year),
            _ => {
                return Err(ListError {
                    message: format!(
                        "invalid --year {raw:?}: expected 1970..=2999"
                    ),
                });
            }
        },
        None => None,
    };
    let mut rows = scan_vault(&vault);
    if let Some(kind) = kind_filter.as_deref() {
        rows.retain(|row| row.kind == kind);
    }
    if let Some(year) = year_filter {
        rows.retain(|row| match row.id {
            PeriodId::Week { year: y, .. }
            | PeriodId::Quarter { year: y, .. } => y == year,
            PeriodId::Year(y) => y == year,
            PeriodId::AllTime => false,
        });
    }
    // Newest first, grouped year → quarter → weeks: years descend,
    // then quarters descend, then weeks descend, with all time last.
    rows.sort_by(|a, b| sort_key(b).cmp(&sort_key(a)));
    let problems = if matches.get_flag("problems") {
        let today = bob_env::current_datetime().date();
        Some(
            super::problems::collect_problems(&vault, today)
                .map_err(|message| ListError { message })?,
        )
    } else {
        None
    };
    Ok((rows, problems))
}

fn kind_of(id: PeriodId) -> &'static str {
    match id {
        PeriodId::Week { .. } => "week",
        PeriodId::Quarter { .. } => "quarter",
        PeriodId::Year(_) => "year",
        PeriodId::AllTime => "all",
    }
}

fn sort_key(row: &ListRow) -> (i32, u8, u32, u32) {
    match row.id {
        PeriodId::Week { year, week } => (year, 1, week, 0),
        PeriodId::Quarter { year, quarter } => (year, 2, quarter, 0),
        PeriodId::Year(year) => (year, 3, 0, 0),
        PeriodId::AllTime => (0, 0, 0, 0),
    }
}

fn scan_vault(vault: &Path) -> Vec<ListRow> {
    let mut rows = Vec::new();
    // Year notes at the root.
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "all_time.md" {
                push_row(vault, &path, PeriodId::AllTime, &mut rows);
            } else if name.len() == 7 && name.ends_with(".md") {
                let stem = &name[..4];
                if let Ok(id) = PeriodId::parse(stem)
                    && matches!(id, PeriodId::Year(_))
                {
                    push_row(vault, &path, id, &mut rows);
                }
            }
        }
    }
    // Week and quarter notes under YYYY/ directories.
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(inner) = fs::read_dir(&path) else {
                continue;
            };
            for entry in inner.flatten() {
                let note_path = entry.path();
                if !note_path.is_file() {
                    continue;
                }
                let Some(name) = note_path.file_name().and_then(|n| n.to_str())
                else {
                    continue;
                };
                if !name.ends_with(".md") {
                    continue;
                }
                let stem = name.trim_end_matches(".md");
                // Week and quarter stems contain a dash; skip dailies.
                if !stem.contains('-') {
                    continue;
                }
                if let Ok(id) = PeriodId::parse(stem)
                    && !matches!(id, PeriodId::Year(_) | PeriodId::AllTime)
                {
                    push_row(vault, &note_path, id, &mut rows);
                }
            }
        }
    }
    rows
}

fn push_row(vault: &Path, path: &Path, id: PeriodId, rows: &mut Vec<ListRow>) {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let parsed = note::parse(&contents);
    let display = path
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    rows.push(ListRow {
        id,
        kind: kind_of(id),
        path: display,
        review_state: parsed.review_state.as_str(),
        headline: parsed.headline,
    });
}

fn print_success(
    rows: &[ListRow],
    problems: Option<&[super::problems::Problem]>,
    problems_only: bool,
    format: OutputFormat,
) {
    match format {
        OutputFormat::Human => {
            if !problems_only {
                let styler = Styler::detect();
                let width = terminal_width();
                for row in rows {
                    let mark = match row.review_state {
                        "reflected" => styler.green("✓"),
                        "drafted" | "edited" => styler.yellow("·"),
                        _ => styler.cyan("–"),
                    };
                    let headline = row.headline.as_deref().unwrap_or_default();
                    let mut line = format!(
                        "{mark} {} {}",
                        row.id,
                        truncate(headline, width.saturating_sub(12))
                    );
                    if headline.is_empty() {
                        line = format!("{mark} {}", row.id);
                    }
                    println!("{line}");
                }
            }
            if let Some(problems) = problems {
                super::problems::print_human(problems);
            }
        }
        OutputFormat::Json => {
            let rows_json: Vec<_> = rows
                .iter()
                .map(|row| {
                    json!({
                        "id": row.id.to_string(),
                        "kind": row.kind,
                        "path": row.path,
                        "review_state": row.review_state,
                        "headline": row.headline,
                    })
                })
                .collect();
            let mut body = json!({
                "ok": true,
                "schema_version": SCHEMA_VERSION,
                "rows": rows_json,
            });
            if let Some(problems) = problems {
                body["problems"] = json!(problems
                    .iter()
                    .map(|p| p.to_json())
                    .collect::<Vec<_>>());
            }
            println!("{}", serde_json::to_string(&body).unwrap_or_default());
        }
    }
}

fn terminal_width() -> usize {
    bob_env::var("COLUMNS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|width| *width >= 20)
        .unwrap_or(80)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    format!(
        "{}…",
        chars[..max_chars.saturating_sub(1)]
            .iter()
            .collect::<String>()
    )
}

fn print_error(error: ListError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
        }
        OutputFormat::Json => {
            let body = json!({
                "ok": false,
                "schema_version": SCHEMA_VERSION,
                "error": {
                    "code": "invalid_period",
                    "message": error.message,
                    "hint": "Pass --kind week|quarter|year and --year as YYYY.",
                },
            });
            println!("{}", serde_json::to_string(&body).unwrap_or_default());
        }
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_orders_newest_first() {
        let rows = vec![
            ListRow {
                id: PeriodId::Week {
                    year: 2026,
                    week: 44,
                },
                kind: "week",
                path: "2026/2026-W44.md".to_string(),
                review_state: "empty",
                headline: None,
            },
            ListRow {
                id: PeriodId::Year(2026),
                kind: "year",
                path: "2026.md".to_string(),
                review_state: "empty",
                headline: None,
            },
            ListRow {
                id: PeriodId::Week {
                    year: 2026,
                    week: 45,
                },
                kind: "week",
                path: "2026/2026-W45.md".to_string(),
                review_state: "empty",
                headline: None,
            },
        ];
        let mut sorted = rows;
        sorted.sort_by(|a, b| sort_key(b).cmp(&sort_key(a)));
        assert_eq!(sorted[0].id.to_string(), "2026");
        assert_eq!(sorted[1].id.to_string(), "2026-W45");
        assert_eq!(sorted[2].id.to_string(), "2026-W44");
    }
}
