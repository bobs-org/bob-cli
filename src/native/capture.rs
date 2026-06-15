use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, IsTerminal, Write},
    iter,
    path::{Path, PathBuf},
};

use chrono::Datelike;
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{env as bob_env, style::Styler};

const COMMAND_NAME: &str = "bob capture";
const INBOX_FILE: &str = "mac_inbox.md";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_capture_error(error, output_format),
    };

    match capture(request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_capture_error(error, output_format),
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

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Capture a task into the Bob vault")
        .long_about(
            "Capture one task into the Bob Obsidian vault.\n\n\
Text is normalized to one line, formatted as a #task with a [created::] stamp, \
and written to mac_inbox.md unless an @route token or --route target is \
provided. Routed tasks are inserted after the last top-level task block in \
<route>.md, creating that route file when needed.",
        )
        .after_help(
            "Examples:\n  bob capture buy milk @groceries\n  echo 'buy milk @groceries' | bob capture\n  bob capture -f json -- @work send status",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
        .arg(text_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Parse, format, and report without writing a file")
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

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Force the route to NAME.md and keep @tokens in text literal")
}

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Task text; multiple args are joined with spaces")
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
struct CaptureRequest {
    bob_dir: PathBuf,
    dry_run: bool,
    forced_route: Option<String>,
    raw_text: String,
}

impl CaptureRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            forced_route: matches.get_one::<String>("route").cloned(),
            raw_text: raw_text_from_matches(matches)?,
        })
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn raw_text_from_matches(matches: &ArgMatches) -> Result<String, CaptureError> {
    if let Some(values) = matches.get_many::<OsString>("text") {
        return Ok(values
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "));
    }

    if io::stdin().is_terminal() {
        return Ok(String::new());
    }

    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| CaptureError::io(format!("read stdin: {error}")))?;
    Ok(line)
}

fn capture(request: CaptureRequest) -> Result<CaptureResult, CaptureError> {
    let parsed =
        parse_capture_text(&request.raw_text, request.forced_route.as_deref())?;
    let created = current_date_string();
    let task_line = format_task_line(&parsed.body, &created);
    let relative_target = relative_target(parsed.route.as_deref());
    let target = request.bob_dir.join(&relative_target);
    let placement = if parsed.route.is_some() {
        capture_routed(&target, &task_line, request.dry_run)?
    } else {
        capture_inbox(&target, &task_line, request.dry_run)?
    };

    Ok(CaptureResult {
        ok: true,
        dry_run: request.dry_run,
        routed: parsed.route.is_some(),
        route_label: parsed
            .route
            .as_deref()
            .map(route_label)
            .unwrap_or_default(),
        route: parsed.route,
        relative_target: relative_target.to_string_lossy().into_owned(),
        target: target.display().to_string(),
        text: parsed.body,
        task_line,
        created,
        placement,
    })
}

fn current_date_string() -> String {
    let now = bob_env::current_datetime();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

fn relative_target(route: Option<&str>) -> PathBuf {
    route
        .map(|route| PathBuf::from(route_label(route)))
        .unwrap_or_else(|| PathBuf::from(INBOX_FILE))
}

fn route_label(route: &str) -> String {
    format!("{route}.md")
}

fn format_task_line(body: &str, created: &str) -> String {
    format!("- [ ] #task {body} [created::{created}]")
}

fn capture_inbox(
    target: &Path,
    task_line: &str,
    dry_run: bool,
) -> Result<Placement, CaptureError> {
    if !target.exists() {
        if !dry_run {
            write_new_file(target, task_line)?;
        }
        return Ok(Placement::Created);
    }

    let contents = read_target(target)?;
    let addition = insertion_text(&contents, contents.len(), task_line);
    if !dry_run {
        append_to_file(target, &addition)?;
    }
    Ok(Placement::Appended)
}

fn capture_routed(
    target: &Path,
    task_line: &str,
    dry_run: bool,
) -> Result<Placement, CaptureError> {
    if !target.exists() {
        if !dry_run {
            write_new_file(target, task_line)?;
        }
        return Ok(Placement::Created);
    }

    let contents = read_target(target)?;
    let (updated, placement) = insert_task_line(&contents, task_line);
    if !dry_run {
        fs::write(target, updated)
            .map_err(|error| fs_error("write target", target, error))?;
    }
    Ok(placement)
}

fn read_target(target: &Path) -> Result<String, CaptureError> {
    fs::read_to_string(target)
        .map_err(|error| fs_error("read target", target, error))
}

fn write_new_file(target: &Path, task_line: &str) -> Result<(), CaptureError> {
    let contents = format!("{task_line}\n");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| fs_error("create target", target, error))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| fs_error("write target", target, error))
}

fn append_to_file(target: &Path, addition: &str) -> Result<(), CaptureError> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(target)
        .map_err(|error| fs_error("open target", target, error))?;
    file.write_all(addition.as_bytes())
        .map_err(|error| fs_error("write target", target, error))
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> CaptureError {
    CaptureError::io(format!("{action} {}: {error}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCaptureText {
    body: String,
    route: Option<String>,
}

fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    let normalized = normalize_task_text(raw_text);
    if normalized.is_empty() {
        return Err(CaptureError::usage(
            "task text is required; pass TEXT or pipe one line on stdin",
        ));
    }

    if let Some(route) = forced_route {
        return Ok(ParsedCaptureText {
            body: normalized,
            route: Some(normalize_forced_route(route)?),
        });
    }

    Ok(parse_auto_route(&normalized))
}

fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_auto_route(text: &str) -> ParsedCaptureText {
    if let Some(rest) = text.strip_prefix('@')
        && let Some((token, body)) = rest.split_once(' ')
        && is_route_token(token)
        && !body.is_empty()
    {
        return ParsedCaptureText {
            body: body.to_string(),
            route: Some(token.to_ascii_lowercase()),
        };
    }

    if let Some(index) = text.rfind(" @") {
        let body = &text[..index];
        let token = &text[index + 2..];
        if !body.is_empty() && is_route_token(token) {
            return ParsedCaptureText {
                body: body.to_string(),
                route: Some(token.to_ascii_lowercase()),
            };
        }
    }

    ParsedCaptureText {
        body: text.to_string(),
        route: None,
    }
}

fn normalize_forced_route(route: &str) -> Result<String, CaptureError> {
    if is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err(CaptureError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

fn is_route_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn insert_task_line(contents: &str, task_line: &str) -> (String, Placement) {
    let Some(index) = last_task_block_insert_index(contents) else {
        return (
            format!(
                "{}{}",
                contents,
                insertion_text(contents, contents.len(), task_line)
            ),
            Placement::Appended,
        );
    };

    let addition = insertion_text(contents, index, task_line);
    let mut updated = String::with_capacity(contents.len() + addition.len());
    updated.push_str(&contents[..index]);
    updated.push_str(&addition);
    updated.push_str(&contents[index..]);
    (updated, Placement::Inserted)
}

fn insertion_text(contents: &str, index: usize, task_line: &str) -> String {
    let needs_leading_newline = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_newline {
        format!("\n{task_line}\n")
    } else {
        format!("{task_line}\n")
    }
}

fn last_task_block_insert_index(contents: &str) -> Option<usize> {
    let lines = line_spans(contents);
    let mut last_index = None;
    for (index, line) in lines.iter().enumerate() {
        if is_top_level_task_line(line.text) {
            last_index = Some(task_block_end(&lines, index));
        }
    }
    last_index
}

fn task_block_end(lines: &[LineSpan<'_>], task_index: usize) -> usize {
    let mut index = task_index + 1;
    while index < lines.len() {
        let line = lines[index].text;
        if is_indented_line(line)
            || (is_blank_line(line)
                && next_nonblank_is_indented(lines, index + 1))
        {
            index += 1;
            continue;
        }
        break;
    }
    lines[index - 1].end
}

fn next_nonblank_is_indented(
    lines: &[LineSpan<'_>],
    start_index: usize,
) -> bool {
    lines[start_index..]
        .iter()
        .find(|line| !is_blank_line(line.text))
        .is_some_and(|line| is_indented_line(line.text))
}

#[derive(Debug, Clone, Copy)]
struct LineSpan<'a> {
    end: usize,
    text: &'a str,
}

fn line_spans(contents: &str) -> Vec<LineSpan<'_>> {
    let mut spans = Vec::new();
    let mut start = 0;
    for segment in contents.split_inclusive('\n') {
        let end = start + segment.len();
        spans.push(LineSpan {
            end,
            text: logical_line(segment),
        });
        start = end;
    }
    spans
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn is_top_level_task_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- [") else {
        return false;
    };
    let mut chars = rest.chars();
    if chars.next().is_none() || chars.next() != Some(']') {
        return false;
    }
    let after_checkbox = chars.as_str();
    after_checkbox
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
        && after_checkbox.contains("#task")
}

fn is_indented_line(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn is_blank_line(line: &str) -> bool {
    line.trim().is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Placement {
    Created,
    Inserted,
    Appended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureResult {
    ok: bool,
    dry_run: bool,
    routed: bool,
    route: Option<String>,
    route_label: String,
    relative_target: String,
    target: String,
    text: String,
    task_line: String,
    created: String,
    placement: Placement,
}

fn print_success(result: &CaptureResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureResult) {
    let styler = Styler::detect();
    let target_label = if result.route_label.is_empty() {
        result.relative_target.as_str()
    } else {
        result.route_label.as_str()
    };
    let target_label = styler.cyan(target_label);
    let verb = if result.dry_run {
        "would capture"
    } else {
        "captured"
    };
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    println!("{prefix} {verb}  {target_label}");
    println!("  {}", styler.dim(&result.task_line));
}

fn success_json(result: &CaptureResult) -> String {
    serde_json::to_string(result).expect("serialize capture result")
}

fn print_capture_error(
    error: CaptureError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureError {
    kind: CaptureErrorKind,
    message: String,
}

impl CaptureError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureErrorKind {
    Usage,
    Io,
}

impl CaptureErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK: &str = "- [ ] #task new thing [created::2026-06-15]";

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(
            normalize_task_text(" \n buy\t  milk \r\n @groceries  "),
            "buy milk @groceries"
        );
    }

    #[test]
    fn parses_auto_routes_like_hammerspoon() {
        let cases = [
            (
                "@Groceries Buy Milk",
                "Buy Milk",
                Some("groceries"),
                "prefix route wins and lower-cases",
            ),
            (
                "Buy Milk @Groceries",
                "Buy Milk",
                Some("groceries"),
                "suffix route lower-cases",
            ),
            ("a @b @C", "a @b", Some("c"), "last suffix token wins"),
            (
                "@Work buy milk @home",
                "buy milk @home",
                Some("work"),
                "prefix wins before suffix",
            ),
            (
                "Email @home soon",
                "Email @home soon",
                None,
                "middle @token stays literal",
            ),
            ("@route", "@route", None, "bare route stays literal"),
            (
                "@bad! body @Good",
                "@bad! body",
                Some("good"),
                "invalid prefix can still use suffix",
            ),
        ];

        for (raw, body, route, label) in cases {
            let parsed =
                parse_capture_text(raw, None).unwrap_or_else(|error| {
                    panic!("{label}: unexpected error: {error:?}")
                });
            assert_eq!(parsed.body, body, "{label}");
            assert_eq!(parsed.route.as_deref(), route, "{label}");
        }
    }

    #[test]
    fn forced_route_bypasses_auto_route_parsing() {
        let parsed =
            parse_capture_text("Buy milk @Groceries", Some("Work-Queue"))
                .expect("parse forced route");
        assert_eq!(parsed.body, "Buy milk @Groceries");
        assert_eq!(parsed.route.as_deref(), Some("work-queue"));

        let error = parse_capture_text("Buy milk", Some("../bad"))
            .expect_err("invalid forced route must fail");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn formats_task_line() {
        assert_eq!(
            format_task_line("buy milk", "2026-06-15"),
            "- [ ] #task buy milk [created::2026-06-15]"
        );
    }

    #[test]
    fn appends_to_empty_and_no_task_files() {
        assert_eq!(
            insert_task_line("", TASK),
            (format!("{TASK}\n"), Placement::Appended)
        );
        assert_eq!(
            insert_task_line("# Header", TASK),
            (format!("# Header\n{TASK}\n"), Placement::Appended)
        );
        assert_eq!(
            insert_task_line("# Header\n", TASK),
            (format!("# Header\n{TASK}\n"), Placement::Appended)
        );
    }

    #[test]
    fn inserts_after_single_top_level_task() {
        let contents = "- [ ] #task old\nPlain paragraph\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [ ] #task old\n{TASK}\nPlain paragraph\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn skips_indented_and_blank_then_indented_continuation_lines() {
        let contents = "- [ ] #task old\n  child\n\n\tdeep\n\nNext\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [ ] #task old\n  child\n\n\tdeep\n{TASK}\n\nNext\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn inserts_after_last_of_many_task_blocks() {
        let contents = "- [ ] #task first\n- [x] #task second\n  note\nTail\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "- [ ] #task first\n- [x] #task second\n  note\n{TASK}\nTail\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn adds_leading_newline_when_inserting_after_non_newline_eof() {
        let contents = "- [B] #task old";
        assert_eq!(
            insert_task_line(contents, TASK),
            (format!("- [B] #task old\n{TASK}\n"), Placement::Inserted,)
        );
    }

    #[test]
    fn inserts_after_final_continuation_running_to_eof() {
        let contents = "- [/] #task old\n  note";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("- [/] #task old\n  note\n{TASK}\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn ignores_indented_task_lines_as_insertion_anchors() {
        let contents = "  - [ ] #task nested";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("  - [ ] #task nested\n{TASK}\n"),
                Placement::Appended,
            )
        );
    }

    #[test]
    fn json_success_shape_is_stable() {
        let result = CaptureResult {
            ok: true,
            dry_run: false,
            routed: true,
            route: Some("groceries".to_string()),
            route_label: "groceries.md".to_string(),
            relative_target: "groceries.md".to_string(),
            target: "/tmp/bob/groceries.md".to_string(),
            text: "buy milk".to_string(),
            task_line: "- [ ] #task buy milk [created::2026-06-15]".to_string(),
            created: "2026-06-15".to_string(),
            placement: Placement::Inserted,
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["dry_run"], false);
        assert_eq!(value["routed"], true);
        assert_eq!(value["route"], "groceries");
        assert_eq!(value["route_label"], "groceries.md");
        assert_eq!(value["relative_target"], "groceries.md");
        assert_eq!(value["target"], "/tmp/bob/groceries.md");
        assert_eq!(value["text"], "buy milk");
        assert_eq!(
            value["task_line"],
            "- [ ] #task buy milk [created::2026-06-15]"
        );
        assert_eq!(value["created"], "2026-06-15");
        assert_eq!(value["placement"], "inserted");
    }
}
