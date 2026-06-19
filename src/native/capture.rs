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
pub(crate) const INBOX_FILE: &str = "mac_inbox.md";

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
        .about("Capture a task or bullet into the Bob vault")
        .long_about(
            "Capture one task into the Bob Obsidian vault.\n\n\
Text is normalized to one line, formatted as a #task with a [created::] stamp, \
and written to mac_inbox.md unless an @route token or --route target is \
provided. Existing target files prefer a Tasks section, then fall back to the \
last top-level task block. Missing target files are created when needed.\n\n\
A terminal '#' or '#<section-prefix>' marker captures an ordinary bullet \
instead. It renders as '- <body> [created::YYYY-MM-DD]' and is placed in a \
non-Tasks section whose heading title starts with the prefix (compared case \
insensitively), or any non-Tasks section for a bare '#'. A matching non-H1 \
section is preferred; a matching H1 heading is used only when no non-H1 \
heading matches. A terminal @route and the '#' marker may appear in either \
order.",
        )
        .after_help(
            "Examples:\n  bob capture buy milk @groceries\n  bob capture jot idea #Ideas @notes\n  echo 'buy milk @groceries' | bob capture\n  bob capture -f json -- @work send status",
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
    let capture_line = match &parsed.kind {
        CaptureKind::Task => format_task_line(&parsed.body, &created),
        CaptureKind::Bullet { .. } => {
            format_bullet_line(&parsed.body, &created)
        }
    };
    let kind_label = capture_kind_label(&parsed.kind);
    let relative_target = relative_target(parsed.route.as_deref());
    let target = request.bob_dir.join(&relative_target);
    let placement = capture_to_target(
        &target,
        &capture_line,
        &parsed.kind,
        request.dry_run,
    )?;

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
        task_line: capture_line,
        kind: kind_label,
        created,
        placement,
    })
}

fn capture_kind_label(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Task => "task",
        CaptureKind::Bullet { .. } => "bullet",
    }
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

pub(crate) fn inbox_route() -> &'static str {
    INBOX_FILE.strip_suffix(".md").unwrap_or(INBOX_FILE)
}

pub(crate) fn route_label(route: &str) -> String {
    format!("{route}.md")
}

fn format_task_line(body: &str, created: &str) -> String {
    format!("- [ ] #task {body} [created::{created}]")
}

fn format_bullet_line(body: &str, created: &str) -> String {
    format!("- {body} [created::{created}]")
}

fn capture_to_target(
    target: &Path,
    capture_line: &str,
    kind: &CaptureKind,
    dry_run: bool,
) -> Result<Placement, CaptureError> {
    if !target.exists() {
        if !dry_run {
            write_new_file(target, capture_line)?;
        }
        return Ok(Placement::Created);
    }

    let contents = read_target(target)?;
    let (updated, placement) = match kind {
        CaptureKind::Task => insert_task_line(&contents, capture_line),
        CaptureKind::Bullet { section_prefix } => insert_bullet_line(
            &contents,
            capture_line,
            section_prefix.as_deref(),
        ),
    };
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

fn write_new_file(
    target: &Path,
    capture_line: &str,
) -> Result<(), CaptureError> {
    let contents = format!("{capture_line}\n");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| fs_error("create target", target, error))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| fs_error("write target", target, error))
}

fn fs_error(action: &str, path: &Path, error: io::Error) -> CaptureError {
    CaptureError::io(format!("{action} {}: {error}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureKind {
    Task,
    Bullet { section_prefix: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCaptureText {
    body: String,
    route: Option<String>,
    kind: CaptureKind,
}

/// Terminal control tokens stripped from the end of a normalized capture.
///
/// `bullet` is the bullet marker: the outer `Option` records whether a terminal
/// `#...` marker was present, and the inner `Option<String>` records its section
/// prefix (`None` for a bare `#`).
struct TerminalControls {
    body: String,
    bullet: Option<Option<String>>,
    route: Option<String>,
}

fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    let normalized = normalize_task_text(raw_text);
    if normalized.is_empty() {
        return Err(missing_text_error());
    }

    let controls =
        peel_terminal_controls(&normalized, forced_route.is_none());

    if let Some(route) = forced_route {
        let route = normalize_forced_route(route)?;
        let Some(section_prefix) = controls.bullet else {
            return Ok(ParsedCaptureText {
                body: normalized,
                route: Some(route),
                kind: CaptureKind::Task,
            });
        };
        if controls.body.is_empty() {
            return Err(missing_text_error());
        }
        return Ok(ParsedCaptureText {
            body: controls.body,
            route: Some(route),
            kind: CaptureKind::Bullet { section_prefix },
        });
    }

    let Some(section_prefix) = controls.bullet else {
        return Ok(parse_auto_route(&normalized));
    };

    if controls.body.is_empty() {
        return Err(missing_text_error());
    }

    let (body, route) = match controls.route {
        Some(route) => (controls.body, Some(route)),
        None => parse_leading_route(&controls.body),
    };
    Ok(ParsedCaptureText {
        body,
        route,
        kind: CaptureKind::Bullet { section_prefix },
    })
}

fn missing_text_error() -> CaptureError {
    CaptureError::usage(
        "task text is required; pass TEXT or pipe one line on stdin",
    )
}

fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Peel a terminal bullet marker and (when `allow_route`) a terminal `@route`
/// token off `normalized`, in either order, consuming at most one of each.
fn peel_terminal_controls(
    normalized: &str,
    allow_route: bool,
) -> TerminalControls {
    let mut tokens: Vec<&str> = normalized.split(' ').collect();
    let mut bullet: Option<Option<String>> = None;
    let mut route: Option<String> = None;

    while let Some(&last) = tokens.last() {
        if bullet.is_none()
            && let Some(rest) = last.strip_prefix('#')
        {
            bullet = Some((!rest.is_empty()).then(|| rest.to_string()));
            tokens.pop();
            continue;
        }

        if allow_route
            && route.is_none()
            && let Some(token) = last.strip_prefix('@')
            && is_route_token(token)
        {
            route = Some(token.to_ascii_lowercase());
            tokens.pop();
            continue;
        }

        break;
    }

    TerminalControls {
        body: tokens.join(" "),
        bullet,
        route,
    }
}

fn parse_auto_route(text: &str) -> ParsedCaptureText {
    let (body, route) = match parse_leading_route(text) {
        (body, Some(route)) => (body, Some(route)),
        (_, None) => parse_trailing_route(text),
    };
    ParsedCaptureText {
        body,
        route,
        kind: CaptureKind::Task,
    }
}

fn parse_leading_route(text: &str) -> (String, Option<String>) {
    if let Some(rest) = text.strip_prefix('@')
        && let Some((token, body)) = rest.split_once(' ')
        && is_route_token(token)
        && !body.is_empty()
    {
        return (body.to_string(), Some(token.to_ascii_lowercase()));
    }

    (text.to_string(), None)
}

fn parse_trailing_route(text: &str) -> (String, Option<String>) {
    if let Some(index) = text.rfind(" @") {
        let body = &text[..index];
        let token = &text[index + 2..];
        if !body.is_empty() && is_route_token(token) {
            return (body.to_string(), Some(token.to_ascii_lowercase()));
        }
    }

    (text.to_string(), None)
}

fn normalize_forced_route(route: &str) -> Result<String, CaptureError> {
    if is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err(CaptureError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

pub(crate) fn is_route_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn insert_task_line(contents: &str, task_line: &str) -> (String, Placement) {
    let lines = line_spans(contents);
    if let Some(section) = tasks_section(&lines) {
        let index = last_task_block_insert_index_in_range(
            &lines,
            section.start_line,
            section.end_line,
        )
        .unwrap_or(section.heading_end);
        let addition = if index == section.heading_end {
            empty_section_insertion_text(contents, index, task_line)
        } else {
            insertion_text(contents, index, task_line)
        };
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    let Some(index) =
        last_task_block_insert_index_in_range(&lines, 0, lines.len())
    else {
        let addition = insertion_text(contents, contents.len(), task_line);
        return (
            insert_at(contents, contents.len(), &addition),
            Placement::Appended,
        );
    };

    let addition = insertion_text(contents, index, task_line);
    (insert_at(contents, index, &addition), Placement::Inserted)
}

fn insert_at(contents: &str, index: usize, addition: &str) -> String {
    let mut updated = String::with_capacity(contents.len() + addition.len());
    updated.push_str(&contents[..index]);
    updated.push_str(&addition);
    updated.push_str(&contents[index..]);
    updated
}

fn insertion_text(contents: &str, index: usize, line: &str) -> String {
    let needs_leading_newline = index > 0 && !contents[..index].ends_with('\n');
    if needs_leading_newline {
        format!("\n{line}\n")
    } else {
        format!("{line}\n")
    }
}

fn empty_section_insertion_text(
    contents: &str,
    index: usize,
    line: &str,
) -> String {
    if index > 0 && contents[..index].ends_with('\n') {
        format!("\n{line}\n")
    } else {
        format!("\n\n{line}\n")
    }
}

fn last_task_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (index, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_task_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + index));
        }
    }
    last_index
}

fn insert_bullet_line(
    contents: &str,
    bullet_line: &str,
    section_prefix: Option<&str>,
) -> (String, Placement) {
    let lines = line_spans(contents);
    let headings = markdown_headings(&lines);
    let section = target_bullet_section(&lines, &headings, section_prefix);

    if let Some(index) = last_bullet_block_insert_index_in_range(
        &lines,
        section.start_line,
        section.end_line,
    ) {
        let addition = insertion_text(contents, index, bullet_line);
        return (insert_at(contents, index, &addition), Placement::Inserted);
    }

    match section.heading_end {
        Some(heading_end) => {
            let addition = empty_section_insertion_text(
                contents,
                heading_end,
                bullet_line,
            );
            (
                insert_at(contents, heading_end, &addition),
                Placement::Inserted,
            )
        }
        None => {
            let index = section.insertion_start;
            let addition = insertion_text(contents, index, bullet_line);
            let placement = if index >= contents.len() {
                Placement::Appended
            } else {
                Placement::Inserted
            };
            (insert_at(contents, index, &addition), placement)
        }
    }
}

/// A Markdown section the bullet capture can target.
///
/// `heading_end` is the byte offset just past the heading line, or `None` for
/// the zeroth (pre-heading) section. `start_line`/`end_line` bound the section
/// body for bullet scanning, and `insertion_start` is where an empty zeroth
/// section receives its first bullet.
#[derive(Debug, Clone, Copy)]
struct MarkdownSection {
    heading_end: Option<usize>,
    start_line: usize,
    end_line: usize,
    insertion_start: usize,
}

fn target_bullet_section(
    lines: &[LineSpan<'_>],
    headings: &[MarkdownHeading<'_>],
    section_prefix: Option<&str>,
) -> MarkdownSection {
    let matches = |heading: &MarkdownHeading<'_>| {
        heading.title != "Tasks"
            && heading_matches_bullet_prefix(heading.title, section_prefix)
    };
    // Prefer the first matching non-H1 heading, falling back to the first
    // matching H1 heading only when no non-H1 heading matches.
    let target = headings
        .iter()
        .position(|heading| heading.level != 1 && matches(heading))
        .or_else(|| {
            headings
                .iter()
                .position(|heading| heading.level == 1 && matches(heading))
        });

    match target {
        Some(pos) => {
            let heading_index = headings[pos].line_index;
            let heading_end = lines[heading_index].end;
            let end_line = headings
                .get(pos + 1)
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: Some(heading_end),
                start_line: heading_index + 1,
                end_line,
                insertion_start: heading_end,
            }
        }
        None => {
            let (start_line, insertion_start) = match frontmatter_span(lines) {
                Some((line_after, byte_end)) => (line_after, byte_end),
                None => (0, 0),
            };
            let end_line = headings
                .first()
                .map(|heading| heading.line_index)
                .unwrap_or(lines.len());
            MarkdownSection {
                heading_end: None,
                start_line,
                end_line,
                insertion_start,
            }
        }
    }
}

/// Whether `title` matches a bullet capture's section prefix. A bare marker
/// (no prefix) matches every heading; otherwise the prefix is compared against
/// the start of `title` case insensitively.
fn heading_matches_bullet_prefix(
    title: &str,
    section_prefix: Option<&str>,
) -> bool {
    match section_prefix {
        None => true,
        Some(prefix) => {
            title.to_lowercase().starts_with(&prefix.to_lowercase())
        }
    }
}

fn last_bullet_block_insert_index_in_range(
    lines: &[LineSpan<'_>],
    start_line: usize,
    end_line: usize,
) -> Option<usize> {
    let mut last_index = None;
    for (offset, line) in lines[start_line..end_line].iter().enumerate() {
        if is_top_level_bullet_line(line.text) {
            last_index = Some(task_block_end(lines, start_line + offset));
        }
    }
    last_index
}

fn is_top_level_bullet_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- ") else {
        return false;
    };
    !is_checkbox_marker(rest)
}

fn is_checkbox_marker(after_dash: &str) -> bool {
    let mut chars = after_dash.chars();
    chars.next() == Some('[')
        && chars.next().is_some()
        && chars.next() == Some(']')
}

/// An ATX heading discovered while scanning a note.
///
/// `line_index` is the heading's line, `level` is its ATX level (number of
/// leading `#`), and `title` is the stripped heading text.
#[derive(Debug, Clone, Copy)]
struct MarkdownHeading<'a> {
    line_index: usize,
    level: usize,
    title: &'a str,
}

/// Collect every ATX heading, skipping YAML frontmatter and fenced code blocks.
fn markdown_headings<'a>(lines: &[LineSpan<'a>]) -> Vec<MarkdownHeading<'a>> {
    let mut headings = Vec::new();
    let mut in_frontmatter = false;
    let mut fence = None;

    for (index, line) in lines.iter().enumerate() {
        if index == 0 && line.text.trim() == "---" {
            in_frontmatter = true;
            continue;
        }

        if in_frontmatter {
            if line.text.trim() == "---" {
                in_frontmatter = false;
            }
            continue;
        }

        if let Some(open_fence) = fence {
            if closes_fence(line.text, open_fence) {
                fence = None;
            }
            continue;
        }

        if let Some(open_fence) = fence_marker(line.text) {
            fence = Some(open_fence);
            continue;
        }

        if let Some((level, title)) = atx_heading(line.text) {
            headings.push(MarkdownHeading {
                line_index: index,
                level,
                title,
            });
        }
    }

    headings
}

/// Byte span of YAML frontmatter as `(line_after, end_byte)` when the document
/// opens with a closed `---` block.
fn frontmatter_span(lines: &[LineSpan<'_>]) -> Option<(usize, usize)> {
    if lines.first().map(|line| line.text.trim()) != Some("---") {
        return None;
    }

    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.text.trim() == "---")
        .map(|(index, line)| (index + 1, line.end))
}

#[derive(Debug, Clone, Copy)]
struct TasksSection {
    heading_end: usize,
    start_line: usize,
    end_line: usize,
}

fn tasks_section(lines: &[LineSpan<'_>]) -> Option<TasksSection> {
    let headings = markdown_headings(lines);
    let pos = headings.iter().position(|heading| heading.title == "Tasks")?;
    let heading_index = headings[pos].line_index;
    let end_line = headings
        .get(pos + 1)
        .map(|heading| heading.line_index)
        .unwrap_or(lines.len());
    Some(TasksSection {
        heading_end: lines[heading_index].end,
        start_line: heading_index + 1,
        end_line,
    })
}

#[derive(Debug, Clone, Copy)]
struct FenceMarker {
    character: u8,
    length: usize,
}

fn fence_marker(line: &str) -> Option<FenceMarker> {
    let (marker, _) = fence_sequence(line)?;
    Some(marker)
}

fn closes_fence(line: &str, open_fence: FenceMarker) -> bool {
    let Some((marker, remainder)) = fence_sequence(line) else {
        return false;
    };

    marker.character == open_fence.character
        && marker.length >= open_fence.length
        && remainder.trim().is_empty()
}

fn fence_sequence(line: &str) -> Option<(FenceMarker, &str)> {
    let line = markdown_indented_line(line)?;
    let bytes = line.as_bytes();
    let character = *bytes.first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }

    let length = bytes.iter().take_while(|byte| **byte == character).count();
    if length < 3 {
        return None;
    }

    Some((FenceMarker { character, length }, &line[length..]))
}

/// Parse an ATX heading into its `(level, title)`, where `level` is the number
/// of leading `#` characters.
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let line = markdown_indented_line(line)?;
    let hashes = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }

    if line
        .as_bytes()
        .get(hashes)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }

    Some((hashes, strip_closing_atx_hashes(line[hashes..].trim())))
}

fn strip_closing_atx_hashes(title: &str) -> &str {
    let trimmed = title.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed;
    }

    if without_hashes
        .chars()
        .next_back()
        .map_or(true, char::is_whitespace)
    {
        without_hashes.trim_end()
    } else {
        trimmed
    }
}

fn markdown_indented_line(line: &str) -> Option<&str> {
    let spaces = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    if spaces > 3 {
        return None;
    }
    Some(&line[spaces..])
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
    kind: &'static str,
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
    const BULLET: &str = "- new idea [created::2026-06-15]";

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
    fn tasks_section_wins_over_root_task_when_empty() {
        let contents = "# Project\n- [ ] #task root\n## Tasks\nNotes\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "# Project\n- [ ] #task root\n## Tasks\n\n{TASK}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn tasks_section_inserts_after_last_task_block_in_section() {
        let contents = concat!(
            "# Project\n",
            "- [ ] #task root\n",
            "## Tasks\n",
            "Intro\n",
            "- [ ] #task old\n",
            "  detail\n",
            "\n",
            "\tmore\n",
            "After\n",
        );
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "{}{TASK}\nAfter\n",
                    concat!(
                        "# Project\n",
                        "- [ ] #task root\n",
                        "## Tasks\n",
                        "Intro\n",
                        "- [ ] #task old\n",
                        "  detail\n",
                        "\n",
                        "\tmore\n",
                    )
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn later_task_outside_tasks_section_does_not_win() {
        let contents =
            "## Tasks\n- [ ] #task in section\n## Other\n- [ ] #task outside\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "## Tasks\n- [ ] #task in section\n{TASK}\n## Other\n- [ ] #task outside\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn ignores_tasks_headings_in_frontmatter_and_fenced_code() {
        let contents = concat!(
            "---\n",
            "# Tasks\n",
            "---\n",
            "```md\n",
            "## Tasks\n",
            "```\n",
            "- [ ] #task old\n",
            "Tail\n",
        );
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!(
                    "---\n\
                     # Tasks\n\
                     ---\n\
                     ```md\n\
                     ## Tasks\n\
                     ```\n\
                     - [ ] #task old\n\
                     {TASK}\n\
                     Tail\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn nested_heading_stops_empty_tasks_section_insertion() {
        let contents = "## Tasks\n### Later\n- [ ] #task later\n";
        assert_eq!(
            insert_task_line(contents, TASK),
            (
                format!("## Tasks\n\n{TASK}\n### Later\n- [ ] #task later\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn tasks_heading_at_eof_inserts_after_blank_line() {
        assert_eq!(
            insert_task_line("## Tasks", TASK),
            (format!("## Tasks\n\n{TASK}\n"), Placement::Inserted,)
        );
        assert_eq!(
            insert_task_line("## Tasks ##\n", TASK),
            (format!("## Tasks ##\n\n{TASK}\n"), Placement::Inserted,)
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
            kind: "task",
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
        assert_eq!(value["kind"], "task");
        assert_eq!(value["created"], "2026-06-15");
        assert_eq!(value["placement"], "inserted");
    }

    #[test]
    fn parses_bullet_marker_with_section_prefix() {
        let parsed =
            parse_capture_text("Some bullet #Ideas", None).expect("parse");
        assert_eq!(parsed.body, "Some bullet");
        assert_eq!(parsed.route, None);
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some("Ideas".to_string())
            }
        );
    }

    #[test]
    fn parses_bare_bullet_marker() {
        let parsed = parse_capture_text("Some bullet #", None).expect("parse");
        assert_eq!(parsed.body, "Some bullet");
        assert_eq!(parsed.route, None);
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: None
            }
        );
    }

    #[test]
    fn terminal_marker_order_is_equivalent() {
        let leading_route =
            parse_capture_text("Some bullet @foo #", None).expect("parse");
        let trailing_route =
            parse_capture_text("Some bullet # @foo", None).expect("parse");
        assert_eq!(leading_route, trailing_route);
        assert_eq!(leading_route.body, "Some bullet");
        assert_eq!(leading_route.route.as_deref(), Some("foo"));
        assert_eq!(
            leading_route.kind,
            CaptureKind::Bullet {
                section_prefix: None
            }
        );
    }

    #[test]
    fn bullet_marker_keeps_leading_route_when_no_terminal_route() {
        let parsed =
            parse_capture_text("@work Some bullet #Ideas", None).expect("parse");
        assert_eq!(parsed.body, "Some bullet");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some("Ideas".to_string())
            }
        );
    }

    #[test]
    fn forced_route_keeps_tokens_literal_but_consumes_bullet_marker() {
        let parsed =
            parse_capture_text("buy milk @groceries #Ideas", Some("Work"))
                .expect("parse");
        assert_eq!(parsed.body, "buy milk @groceries");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some("Ideas".to_string())
            }
        );
    }

    #[test]
    fn marker_only_bullet_input_is_usage_error() {
        let error = parse_capture_text("#", None).expect_err("marker only");
        assert_eq!(error.kind, CaptureErrorKind::Usage);

        let error = parse_capture_text("#", Some("Work"))
            .expect_err("forced marker only");
        assert_eq!(error.kind, CaptureErrorKind::Usage);
    }

    #[test]
    fn formats_bullet_line() {
        assert_eq!(
            format_bullet_line("some idea", "2026-06-15"),
            "- some idea [created::2026-06-15]"
        );
    }

    #[test]
    fn bullet_inserts_after_matched_section_header() {
        let contents = "# Notes\n## Ideas\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("# Notes\n## Ideas\n\n{BULLET}\nNotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_inserts_after_last_ordinary_bullet_block() {
        let contents = "## Ideas\n- first\n  detail\n\n\tmore\nAfter\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "## Ideas\n- first\n  detail\n\n\tmore\n{BULLET}\nAfter\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_treats_checkbox_only_section_as_empty() {
        let contents = "## Ideas\n- [ ] #task t\n- [x] done\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("## Ideas\n\n{BULLET}\n- [ ] #task t\n- [x] done\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_skips_tasks_section_matching_prefix() {
        let contents = "## Tasks\n- [ ] #task t\n## Ta-da\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ta")),
            (
                format!("## Tasks\n- [ ] #task t\n## Ta-da\n\n{BULLET}\nNotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bare_bullet_marker_selects_first_non_tasks_section() {
        let contents = "## Tasks\n- [ ] #task t\n## Ideas\nNotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, None),
            (
                format!("## Tasks\n- [ ] #task t\n## Ideas\n\n{BULLET}\nNotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn unmatched_prefix_falls_back_to_zeroth_section() {
        let contents = "Intro line\n## Tasks\n- [ ] #task t\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!("{BULLET}\nIntro line\n## Tasks\n- [ ] #task t\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn zeroth_section_insertion_after_frontmatter() {
        let contents =
            "---\ntype: area\n---\nIntro\n## Tasks\n- [ ] #task t\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "---\ntype: area\n---\n{BULLET}\nIntro\n## Tasks\n- [ ] #task t\n"
                ),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_prefers_non_h1_match_over_earlier_h1_match() {
        let contents = "# Roadmap\nintro\n\n## Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("R")),
            (
                format!("# Roadmap\nintro\n\n## Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_uses_h1_match_when_no_non_h1_match_exists() {
        let contents = "# Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("R")),
            (
                format!("# Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bare_bullet_marker_prefers_non_h1_section() {
        let contents = "# Title\nintro\n\n## Notes\nbody\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, None),
            (
                format!("# Title\nintro\n\n## Notes\n\n{BULLET}\nbody\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_section_prefix_matches_case_insensitively() {
        let contents = "## Research\nnotes\n";
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("r")),
            (
                format!("## Research\n\n{BULLET}\nnotes\n"),
                Placement::Inserted,
            )
        );
    }

    #[test]
    fn bullet_ignores_headings_in_frontmatter_and_fences() {
        let contents = concat!(
            "---\n",
            "## Ideas\n",
            "---\n",
            "```md\n",
            "## Ideas\n",
            "```\n",
            "## Ideas\n",
            "Notes\n",
        );
        assert_eq!(
            insert_bullet_line(contents, BULLET, Some("Ideas")),
            (
                format!(
                    "---\n## Ideas\n---\n```md\n## Ideas\n```\n## Ideas\n\n{BULLET}\nNotes\n"
                ),
                Placement::Inserted,
            )
        );
    }
}
