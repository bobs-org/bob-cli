use std::{
    env,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use super::env as bob_env;

#[derive(Debug)]
pub(crate) struct Error {
    code: i32,
    message: String,
}

impl Error {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Default)]
struct Options {
    verbose: bool,
}

#[derive(Debug)]
struct LedgerPomodoro {
    start: String,
    end: String,
    range: String,
    task: String,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let options = match parse_args(args) {
        ParseResult::Run(options) => options,
        ParseResult::Help => {
            print_help();
            return 0;
        }
        ParseResult::Error(error) => {
            print_error(&error);
            return error.code;
        }
    };

    match status(options.verbose) {
        Ok(Some(output)) => {
            println!("{output}");
            0
        }
        Ok(None) => 0,
        Err(error) => {
            print_error(&error);
            error.code
        }
    }
}

pub(crate) fn run_tmux(args: Vec<OsString>) -> i32 {
    match parse_tmux_args(args) {
        ParseResult::Run(_) => {}
        ParseResult::Help => {
            print_tmux_help();
            return 0;
        }
        ParseResult::Error(error) => {
            print_tmux_error(&error);
            return error.code;
        }
    }

    if let Ok(Some(output)) = status_from_env()
        && !output.is_empty()
    {
        print!("{output} | ");
    }

    0
}

pub(crate) fn status_from_env() -> Result<Option<String>, Error> {
    status(false)
}

fn status(verbose: bool) -> Result<Option<String>, Error> {
    let day_file = day_file();
    if !day_file.is_file() {
        debug(
            verbose,
            format_args!("Bob day file does not exist: {}", day_file.display()),
        );
        return Ok(None);
    }

    let ledger_pomodoro =
        latest_ledger_pomodoro(&day_file).map_err(|error| {
            Error::new(
                1,
                format!(
                    "failed to read Bob day file {}: {error}",
                    day_file.display()
                ),
            )
        })?;

    let Some(ledger_pomodoro) = ledger_pomodoro else {
        debug(
            verbose,
            format_args!(
                "No Bob ledger pomodoros found in {}",
                day_file.display()
            ),
        );
        return Ok(None);
    };

    debug(
        verbose,
        format_args!(
            "Latest Bob ledger pomodoro: {}-{} {}",
            ledger_pomodoro.start, ledger_pomodoro.end, ledger_pomodoro.task
        ),
    );

    let day_date = day_date(&day_file);
    let end_time = bob_env::parse_hhmm(&ledger_pomodoro.end)
        .ok_or_else(|| Error::new(1, "failed to parse Pomodoro end time"))?;
    let end_datetime = NaiveDateTime::new(day_date, end_time);
    let now = bob_env::current_datetime();
    let now_minus_end = now.signed_duration_since(end_datetime).num_seconds();
    let output = format_pomodoro(&ledger_pomodoro.range, &ledger_pomodoro.task);

    if now_minus_end > 0 {
        if now_minus_end < 600 {
            let overdue_minutes = now_minus_end / 60;
            return Ok(Some(format!(
                "[OVERDUE by {overdue_minutes}m] {output}"
            )));
        }

        return Ok(None);
    }

    let end_minus_now = end_datetime.signed_duration_since(now).num_seconds();
    let minutes_until_due = (end_minus_now / 60) + 1;
    Ok(Some(format!("[<{minutes_until_due}m] {output}")))
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    let mut options = Options::default();

    for arg in args {
        let arg = bob_env::os_to_string(&arg);
        match arg.as_str() {
            "-h" | "--help" => return ParseResult::Help,
            "-d" | "--debug" | "-v" | "--verbose" => {
                options.verbose = true;
            }
            _ => {
                return ParseResult::Error(Error::new(
                    2,
                    format!("Unexpected argument: {arg}"),
                ));
            }
        }
    }

    ParseResult::Run(options)
}

fn parse_tmux_args(args: Vec<OsString>) -> ParseResult {
    if args.iter().any(|arg| {
        let value = arg.as_os_str();
        value == OsStr::new("--help") || value == OsStr::new("-h")
    }) {
        return ParseResult::Help;
    }

    if let Some(arg) = args.first() {
        return ParseResult::Error(Error::new(
            2,
            format!("Unexpected argument: {}", arg.to_string_lossy()),
        ));
    }

    ParseResult::Run(Options::default())
}

enum ParseResult {
    Run(Options),
    Help,
    Error(Error),
}

fn day_file() -> PathBuf {
    env::var_os("BOB_DAY_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| bob_env::default_day_file(&bob_env::bob_dir()))
}

fn day_date(day_file: &Path) -> NaiveDate {
    if let Some(file_name) = day_file.file_name().and_then(|name| name.to_str())
        && let Some(date) = parse_day_file_date(file_name)
    {
        return date;
    }

    bob_env::current_datetime().date()
}

fn parse_day_file_date(file_name: &str) -> Option<NaiveDate> {
    let date_text = file_name.strip_suffix("_day.md")?;
    if date_text.len() != 8
        || !date_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let year = date_text[..4].parse().ok()?;
    let month = date_text[4..6].parse().ok()?;
    let day = date_text[6..8].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn latest_ledger_pomodoro(
    day_file: &Path,
) -> io::Result<Option<LedgerPomodoro>> {
    let contents = fs::read_to_string(day_file)?;
    let mut in_pomodoros = false;
    let mut last = None;

    for line in contents.lines() {
        if is_pomodoros_heading(line) {
            in_pomodoros = true;
            continue;
        }

        if in_pomodoros && is_level_two_heading(line) {
            in_pomodoros = false;
        }

        if !in_pomodoros {
            continue;
        }

        if let Some(task) = open_ledger_task(line)
            && let Some((raw_range, start, end)) = task_time_range(task)
        {
            let cleaned_task = clean_task(task, raw_range);
            let range = format!("{start}-{end}");
            last = Some(LedgerPomodoro {
                start,
                end,
                range,
                task: cleaned_task,
            });
        }
    }

    Ok(last)
}

fn is_pomodoros_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    let rest = trim_one_or_more_spaces(rest);
    let Some(rest) = rest.and_then(|text| text.strip_prefix("Pomodoros"))
    else {
        return false;
    };

    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

fn is_level_two_heading(line: &str) -> bool {
    line.strip_prefix("##")
        .and_then(trim_one_or_more_spaces)
        .is_some()
}

fn trim_one_or_more_spaces(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    (trimmed.len() < value.len()).then_some(trimmed)
}

fn open_ledger_task(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let rest = line.strip_prefix('-')?;
    let rest = trim_one_or_more_spaces(rest)?;

    if let Some(after_open) = rest.strip_prefix('[') {
        let close_index = after_open.find(']')?;
        let checkbox = &after_open[..close_index];
        let task = trim_one_or_more_spaces(&after_open[close_index + 1..])?;
        if checkbox.trim().eq_ignore_ascii_case("x") {
            return None;
        }
        return Some(task.trim_end());
    }

    Some(rest.trim_end())
}

fn task_time_range(task: &str) -> Option<(&str, String, String)> {
    let mut search_start = 0;
    while let Some(open_offset) = task[search_start..].find('(') {
        let open = search_start + open_offset;
        let after_open = open + '('.len_utf8();
        let close_offset = task[after_open..].find(')')?;
        let close = after_open + close_offset;
        let inside = &task[after_open..close];

        if let Some((start, end)) = parse_parenthetical_time_range(inside) {
            let raw_range = &task[open..close + ')'.len_utf8()];
            return Some((raw_range, start, end));
        }

        search_start = close + ')'.len_utf8();
    }

    None
}

fn parse_parenthetical_time_range(inside: &str) -> Option<(String, String)> {
    let (raw_start, raw_end) = inside.split_once('-')?;
    let raw_start = raw_start.trim();
    let (raw_start, bold) = if let Some(start) = raw_start.strip_prefix("**") {
        (start, true)
    } else {
        (raw_start, false)
    };
    let start = normalized_hhmm(raw_start.trim())?;
    let end = normalized_leading_hhmm(raw_end, bold)?;

    Some((start, end))
}

fn normalized_hhmm(value: &str) -> Option<String> {
    let time = bob_env::parse_hhmm(value)?;
    Some(format!("{:02}{:02}", time.hour(), time.minute()))
}

fn normalized_leading_hhmm(value: &str, bold: bool) -> Option<String> {
    let trimmed = value.trim_start();
    let end = trimmed
        .find(|character: char| {
            !(character.is_ascii_digit() || character == ':')
        })
        .unwrap_or(trimmed.len());

    if end == 0 {
        return None;
    }

    let mut rest = &trimmed[end..];
    if bold {
        rest = rest.strip_prefix("**")?;
    } else if rest.starts_with("**") {
        return None;
    }

    if !rest.is_empty() && !rest.chars().next().is_some_and(char::is_whitespace)
    {
        return None;
    }

    normalized_hhmm(&trimmed[..end])
}

fn clean_task(task: &str, raw_range: &str) -> String {
    let without_range = task.replacen(raw_range, "", 1);
    let without_bracket_fields = remove_field_links(&without_range);
    compress_whitespace(&without_bracket_fields)
}

fn remove_field_links(task: &str) -> String {
    let mut output = String::with_capacity(task.len());
    let mut index = 0;

    while index < task.len() {
        let remaining = &task[index..];
        let Some(open_offset) = remaining.find('[') else {
            output.push_str(remaining);
            break;
        };

        let open = index + open_offset;
        output.push_str(&task[index..open]);

        let double_bracket = task[open..].starts_with("[[");
        let content_start = open + if double_bracket { 2 } else { 1 };
        let close_pattern = if double_bracket { "]]" } else { "]" };
        let Some(close_offset) = task[content_start..].find(close_pattern)
        else {
            output.push_str(&task[open..]);
            break;
        };

        let close = content_start + close_offset + close_pattern.len();
        let content = &task[content_start..content_start + close_offset];
        if is_field_link(content) {
            index = close;
        } else {
            output.push_str(&task[open..close]);
            index = close;
        }
    }

    output
}

fn is_field_link(content: &str) -> bool {
    let Some((name, _)) = content.split_once("::") else {
        return false;
    };

    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
        })
}

fn compress_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_pomodoro(range: &str, task: &str) -> String {
    if task.is_empty() {
        range.to_string()
    } else {
        format!("{range} {task}")
    }
}

fn debug(enabled: bool, args: std::fmt::Arguments<'_>) {
    if enabled {
        eprintln!("bob_pomodoro: debug: {args}");
    }
}

fn print_error(error: &Error) {
    eprintln!("bob_pomodoro: error: {}", error.message);
}

fn print_tmux_error(error: &Error) {
    eprintln!("tmux_bob_pomodoro: error: {}", error.message);
    eprintln!("Try 'bob tmux-pomodoro --help' for more information.");
}

fn print_help() {
    println!(
        "\
usage: bob pomodoro [-d|--debug] [-v|--verbose]
       bob pomodoro -h

Show the current Pomodoro status from today's Bob daily note.

The command prints the latest open Pomodoro ledger entry with the remaining
time or recent overdue status. It exits successfully with no output when the
daily note is missing, has no open Pomodoro, or the Pomodoro is more than nine
minutes overdue.

environment:
  BOB_DAY_FILE  exact daily note path to read
  BOB_DIR       Bob vault root used to find the default daily note
  BOB_NOW       override the current timestamp for status calculations

options:
  -d, --debug    enable debug tracing
  -h, --help     show this help message and exit
  -v, --verbose  enable verbose debug output"
    );
}

fn print_tmux_help() {
    println!(
        "\
usage: bob tmux-pomodoro
       bob tmux-pomodoro -h

Print the current Pomodoro status in tmux status-line format.

When an active or recently overdue Pomodoro exists, the output is the regular
Pomodoro status followed by ` | `. Missing or stale Pomodoros produce no
output.

environment:
  BOB_DAY_FILE  exact daily note path to read
  BOB_DIR       Bob vault root used to find the default daily note
  BOB_NOW       override the current timestamp for status calculations

options:
  -h, --help     show this help message and exit"
    );
}
