use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::env as bob_env;

const SCRIPT_NAME: &str = "bob_pomodoro_runtimes";
const STOPWATCH: &str = "\u{23f1}\u{fe0f}";
const BARE_STOPWATCH: &str = "\u{23f1}";
const SYNC_ALREADY_RUNNING_MESSAGE: &str =
    "Another sync instance is already running for this vault.";

#[derive(Debug, Default)]
struct Args {
    check: bool,
    notes: Vec<PathBuf>,
}

#[derive(Debug)]
struct NotePath {
    display: PathBuf,
    path: PathBuf,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let args = match parse_args(args) {
        ParseResult::Run(args) => args,
        ParseResult::Help => {
            print_help();
            return 0;
        }
        ParseResult::Error(message) => {
            eprintln!("{SCRIPT_NAME}: {message}");
            eprintln!("Try '{SCRIPT_NAME} --help' for more information.");
            return 2;
        }
    };

    let bob_dir = bob_env::bob_dir();
    let note_paths = note_paths(&bob_dir, args.notes);

    if !run_ob_sync(&bob_dir) {
        return 2;
    }

    let mut changed_paths = Vec::new();
    for note_path in note_paths {
        match process_note(&note_path.path, args.check) {
            Ok(changed) if changed => changed_paths.push(note_path.display),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "{SCRIPT_NAME}: note not found: {}",
                    note_path.display.display()
                );
                return 2;
            }
            Err(error) => {
                eprintln!(
                    "{SCRIPT_NAME}: failed to process {}: {error}",
                    note_path.display.display()
                );
                return 1;
            }
        }
    }

    if args.check && !changed_paths.is_empty() {
        for path in changed_paths {
            eprintln!("{SCRIPT_NAME}: would update: {}", path.display());
        }
        return 1;
    }

    for path in changed_paths {
        println!("{SCRIPT_NAME}: updated: {}", path.display());
    }

    0
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    let mut parsed = Args::default();
    let mut positional = false;

    for arg in args {
        if !positional {
            let text = bob_env::os_to_string(&arg);
            match text.as_str() {
                "-h" | "--help" => return ParseResult::Help,
                "--check" => {
                    parsed.check = true;
                    continue;
                }
                "--" => {
                    positional = true;
                    continue;
                }
                _ if text.starts_with('-') => {
                    return ParseResult::Error(format!(
                        "unrecognized argument: {text}"
                    ));
                }
                _ => {}
            }
        }

        parsed.notes.push(PathBuf::from(arg));
    }

    ParseResult::Run(parsed)
}

enum ParseResult {
    Run(Args),
    Help,
    Error(String),
}

fn print_help() {
    println!(
        "\
usage: {SCRIPT_NAME} [-h] [--check] [notes ...]

Annotate completed Bob Pomodoro ledger entries with runtimes.

positional arguments:
  notes       note paths to update; defaults to today's Bob daily note

options:
  -h, --help  show this help message and exit
  --check     report whether notes would change without writing them"
    );
}

fn note_paths(bob_dir: &Path, notes: Vec<PathBuf>) -> Vec<NotePath> {
    if notes.is_empty() {
        let path = bob_env::default_day_file(bob_dir);
        return vec![NotePath {
            display: path.clone(),
            path,
        }];
    }

    notes
        .into_iter()
        .map(|display| NotePath {
            path: bob_env::expand_tilde(&display),
            display,
        })
        .collect()
}

fn run_ob_sync(bob_dir: &Path) -> bool {
    let ob_command = env::var_os("OB_COMMAND")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("ob"));

    let output = Command::new(&ob_command)
        .arg("sync")
        .arg("--path")
        .arg(bob_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
        Err(error) => {
            eprintln!("{SCRIPT_NAME}: failed to run ob sync: {error}");
            return false;
        }
    };

    if output.status.success() {
        return true;
    }

    let sync_output = merged_output(&output);
    if sync_output.contains(SYNC_ALREADY_RUNNING_MESSAGE) {
        return true;
    }

    write_stderr_output(&sync_output);
    eprintln!(
        "{SCRIPT_NAME}: ob sync failed with exit code {}",
        bob_env::exit_code(output.status)
    );
    false
}

fn process_note(path: &Path, check: bool) -> io::Result<bool> {
    let contents = fs::read_to_string(path)?;
    let lines = lines_with_endings(&contents);
    let new_lines = updated_lines(&lines);
    let changed = new_lines != lines;

    if changed && !check {
        fs::write(path, new_lines.concat())?;
    }

    Ok(changed)
}

fn updated_lines(lines: &[String]) -> Vec<String> {
    let Some((heading_index, end_index)) = find_pomodoros_section(lines) else {
        return lines.to_vec();
    };

    let mut new_lines = lines.to_vec();
    for line in new_lines.iter_mut().take(end_index).skip(heading_index + 1) {
        let (content, newline) = split_line_ending(line);
        let content = content.to_string();
        let newline = newline.to_string();
        if !is_completed_ledger(&content) {
            continue;
        }

        let stripped = strip_runtime_suffix(&content);
        let Some(range) = find_time_range(&stripped) else {
            *line = format!("{stripped}{newline}");
            continue;
        };
        let minutes = runtime_minutes(range.start_minutes, range.end_minutes);

        let before_range = &stripped[..range.open + '('.len_utf8()];
        let after_range = &stripped[range.close..];
        *line = format!(
            "{before_range}{} {STOPWATCH} {}{after_range}{newline}",
            range.text,
            format_runtime(minutes)
        );
    }

    let (heading, newline) = split_line_ending(&new_lines[heading_index]);
    if let Some(base_heading) = pomodoros_heading_base(heading) {
        new_lines[heading_index] = format!("{base_heading}{newline}");
    }

    new_lines
}

fn find_pomodoros_section(lines: &[String]) -> Option<(usize, usize)> {
    let heading_index = lines.iter().position(|line| {
        let (content, _) = split_line_ending(line);
        is_pomodoros_heading(content)
    })?;

    let end_index = lines
        .iter()
        .enumerate()
        .skip(heading_index + 1)
        .find_map(|(index, line)| {
            let (content, _) = split_line_ending(line);
            is_level_two_heading(content).then_some(index)
        })
        .unwrap_or(lines.len());

    Some((heading_index, end_index))
}

fn lines_with_endings(contents: &str) -> Vec<String> {
    contents.split_inclusive('\n').map(str::to_string).collect()
}

fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        return (content, "\r\n");
    }
    if let Some(content) = line.strip_suffix('\n') {
        return (content, "\n");
    }
    (line, "")
}

fn is_pomodoros_heading(line: &str) -> bool {
    pomodoros_heading_base(line)
        .map(|base| {
            let rest = &line[base.len()..];
            rest.is_empty() || rest.starts_with(char::is_whitespace)
        })
        .unwrap_or(false)
}

fn pomodoros_heading_base(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("##")?;
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None;
    }

    let offset = line.len() - trimmed.len();
    let after_word = trimmed.strip_prefix("Pomodoros")?;
    let word_end = line.len() - after_word.len();
    Some(&line[..word_end.max(offset + "Pomodoros".len())])
}

fn is_level_two_heading(line: &str) -> bool {
    line.strip_prefix("##")
        .map(|rest| rest.trim_start().len() < rest.len())
        .unwrap_or(false)
}

fn is_completed_ledger(line: &str) -> bool {
    let line = line.trim_start();
    let Some(bullet) = line.chars().next() else {
        return false;
    };
    if !matches!(bullet, '-' | '*' | '+') {
        return false;
    }

    let rest = &line[bullet.len_utf8()..];
    if rest.trim_start().len() == rest.len() {
        return false;
    }
    let rest = rest.trim_start();
    let checkbox = rest.get(..3);
    if !matches!(checkbox, Some("[x]" | "[X]")) {
        return false;
    }

    rest.get(3..)
        .map(|after| after.starts_with(char::is_whitespace))
        .unwrap_or(false)
}

fn strip_runtime_suffix(line: &str) -> String {
    let trimmed = line.trim_end();

    if let Some(stripped) = strip_legacy_runtime_suffix(trimmed) {
        return stripped.to_string();
    }

    if let Some(stripped) = strip_stopwatch_runtime_suffix(trimmed) {
        return stripped.to_string();
    }

    line.to_string()
}

fn strip_legacy_runtime_suffix(line: &str) -> Option<&str> {
    if !line.ends_with(']') {
        return None;
    }

    let open = line.rfind("[runtime::")?;
    let prefix = &line[..open];
    prefix.chars().last().filter(|char| char.is_whitespace())?;
    Some(prefix.trim_end())
}

fn strip_stopwatch_runtime_suffix(line: &str) -> Option<&str> {
    let (before_duration, duration) = split_last_whitespace(line)?;
    if duration.is_empty() {
        return None;
    }

    let before_duration = before_duration.trim_end();
    if let Some(before_stopwatch) = before_duration.strip_suffix(STOPWATCH) {
        if has_unclosed_parenthesis(before_stopwatch) {
            return None;
        }
        return Some(before_stopwatch.trim_end());
    }
    if let Some(before_stopwatch) = before_duration.strip_suffix(BARE_STOPWATCH)
    {
        if has_unclosed_parenthesis(before_stopwatch) {
            return None;
        }
        return Some(before_stopwatch.trim_end());
    }

    None
}

fn has_unclosed_parenthesis(value: &str) -> bool {
    let last_open = value.rfind('(');
    let last_close = value.rfind(')');

    matches!(last_open, Some(open) if last_close.map_or(true, |close| close < open))
}

fn split_last_whitespace(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_end();
    let index = value
        .char_indices()
        .rev()
        .find_map(|(index, char)| char.is_whitespace().then_some(index))?;
    Some((&value[..index], value[index..].trim_start()))
}

#[derive(Debug)]
struct TimeRange<'a> {
    open: usize,
    close: usize,
    text: &'a str,
    start_minutes: i32,
    end_minutes: i32,
}

fn find_time_range(line: &str) -> Option<TimeRange<'_>> {
    let mut search_start = 0;
    while let Some(open_offset) = line[search_start..].find('(') {
        let open = search_start + open_offset;
        let after_open = open + '('.len_utf8();
        let close_offset = line[after_open..].find(')')?;
        let close = after_open + close_offset;
        let inside = &line[after_open..close];

        if let Some(parsed) = parse_time_range_contents(inside) {
            return Some(TimeRange {
                open,
                close,
                text: parsed.text,
                start_minutes: parsed.start_minutes,
                end_minutes: parsed.end_minutes,
            });
        }

        search_start = close + ')'.len_utf8();
    }

    None
}

struct ParsedTimeRange<'a> {
    text: &'a str,
    start_minutes: i32,
    end_minutes: i32,
}

fn parse_time_range_contents(inside: &str) -> Option<ParsedTimeRange<'_>> {
    let start = first_non_whitespace(inside)?;
    let after_leading = &inside[start..];
    let (start_minutes, start_len) = parse_time_at_start(after_leading)?;
    let after_start_index = start + start_len;
    let after_start = &inside[after_start_index..];
    let hyphen_offset = first_non_whitespace(after_start)?;
    if after_start[hyphen_offset..].chars().next()? != '-' {
        return None;
    }
    let after_hyphen_index = after_start_index + hyphen_offset + '-'.len_utf8();
    let after_hyphen = &inside[after_hyphen_index..];
    let end_offset = first_non_whitespace(after_hyphen)?;
    let end_start_index = after_hyphen_index + end_offset;
    let (end_minutes, end_len) =
        parse_time_at_start(&inside[end_start_index..])?;
    let end_index = end_start_index + end_len;

    Some(ParsedTimeRange {
        text: inside[start..end_index].trim_end(),
        start_minutes,
        end_minutes,
    })
}

fn first_non_whitespace(value: &str) -> Option<usize> {
    value
        .char_indices()
        .find_map(|(index, char)| (!char.is_whitespace()).then_some(index))
}

fn parse_time_at_start(value: &str) -> Option<(i32, usize)> {
    let bytes = value.as_bytes();
    let (hours, minutes, len) = if bytes.get(..5).is_some_and(is_colon_time) {
        (&value[..2], &value[3..5], 5)
    } else if bytes.get(..4).is_some_and(is_compact_time) {
        (&value[..2], &value[2..4], 4)
    } else {
        return None;
    };

    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }

    Some((hours * 60 + minutes, len))
}

fn is_colon_time(bytes: &[u8]) -> bool {
    bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

fn is_compact_time(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_digit)
}

fn runtime_minutes(start: i32, mut end: i32) -> i32 {
    if end < start {
        end += 24 * 60;
    }

    end - start
}

fn format_runtime(minutes: i32) -> String {
    let hours = minutes / 60;
    let minutes = minutes % 60;

    match (hours, minutes) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h{minutes}m"),
    }
}

fn merged_output(output: &std::process::Output) -> String {
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_stderr_output(output: &str) {
    if output.is_empty() {
        return;
    }

    eprint!("{output}");
    if !output.ends_with('\n') {
        eprintln!();
    }
}
