use std::{
    env,
    ffi::OsString,
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
        Ok(options) => options,
        Err(error) => {
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
    if !args.is_empty() {
        return 0;
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

fn parse_args(args: Vec<OsString>) -> Result<Options, Error> {
    let mut options = Options::default();

    for arg in args {
        let arg = bob_env::os_to_string(&arg);
        match arg.as_str() {
            "-d" | "--debug" | "-v" | "--verbose" => {
                options.verbose = true;
            }
            _ => {
                return Err(Error::new(
                    2,
                    format!("Unexpected argument: {arg}"),
                ));
            }
        }
    }

    Ok(options)
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
            && let Some((raw_start, raw_end, start, end)) =
                task_time_range(task)
        {
            let cleaned_task = clean_task(task, raw_start, raw_end);
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

fn task_time_range(task: &str) -> Option<(&str, &str, String, String)> {
    let mut search_start = 0;
    while let Some(open_offset) = task[search_start..].find('(') {
        let open = search_start + open_offset;
        let after_open = open + '('.len_utf8();
        let close_offset = task[after_open..].find(')')?;
        let close = after_open + close_offset;
        let inside = &task[after_open..close];

        if let Some((raw_start, raw_end)) = inside.split_once('-')
            && let (Some(start), Some(end)) = (
                normalized_hhmm(raw_start.trim()),
                normalized_hhmm(raw_end.trim()),
            )
        {
            return Some((raw_start, raw_end, start, end));
        }

        search_start = close + ')'.len_utf8();
    }

    None
}

fn normalized_hhmm(value: &str) -> Option<String> {
    let time = bob_env::parse_hhmm(value)?;
    Some(format!("{:02}{:02}", time.hour(), time.minute()))
}

fn clean_task(task: &str, raw_start: &str, raw_end: &str) -> String {
    let without_range =
        task.replacen(&format!("({raw_start}-{raw_end})"), "", 1);
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
