use std::{
    ffi::OsString,
    fs,
    io::{self, IsTerminal, Read},
    iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture,
    capture_language::{self, CompletionContext},
    capture_links::{
        self, WikilinkBlockCandidate, WikilinkHeadingCandidate,
        WikilinkNoteCandidate,
    },
    capture_targets::{self, CaptureTargetKind},
    capture_tasks, env as bob_env, note_tasks,
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-complete";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let bob_dir = bob_dir_from_matches(&matches);
    let cursor = *matches.get_one::<usize>("cursor").expect("required");

    let raw_text = match raw_text_from_matches(&matches) {
        Ok(raw_text) => raw_text,
        Err(error) => {
            return print_error(&CompleteError::usage(error), output_format);
        }
    };
    if cursor > raw_text.len() || !raw_text.is_char_boundary(cursor) {
        return print_error(
            &CompleteError::usage(
                "--cursor must be a UTF-8 byte boundary within TEXT",
            ),
            output_format,
        );
    }

    match build_result(&bob_dir, &raw_text, cursor) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(&error, output_format),
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
        .about("Complete capture or wikilink syntax at the cursor")
        .long_about(
            "Return cursor-aware completion candidates for in-progress \
capture TEXT.\n\n\
It shares the phase-grammar tokenizer and `@token` classification with \
`bob capture-parse`, so a completion can never disagree with the marker \
highlighting derived from that command. TEXT accepts the same multi-line \
authored-bullet draft `bob capture` does; completion always scopes to the \
physical line the cursor is on, so only the first (parent) line offers a \
leading marker and a later column-zero or valid two-space nested authored \
line only completes its own trailing marker. The \
service decides whether completion applies at all: an unrecognized marker, \
a cursor in plain body text, a cursor on an authored line's indentation or \
bullet marker itself, an orphaned nested line, or a cursor on a token in the middle of a line all return a \
successful empty result rather than an error.\n\n\
Route completion covers a bare '@', a still-typing '@fragment', and the \
missing route portion of '@^...', '@+...', '@:...', and '@#...', backed by the same \
scan as `bob capture-targets`. Section completion covers '@route#prefix', \
backed by the same scan as `bob capture-sections`. Pomodoro block-ID \
completion covers '@route:prefix' and parent-task completion covers \
'@route+prefix', both backed by the same open-task scan as \
`bob capture-tasks`. The authored ID portion of '@route^block-id' has no \
completion source and returns an empty success. Candidates rank exact prefix matches before substring \
matches, case-insensitively, while keeping each discovery source's stable \
order.\n\n\
When the cursor is inside an Obsidian wikilink, wikilink completion takes \
precedence over capture-marker completion. Note completion covers `[[note` \
and offers Markdown note paths, stems, and aliases. Heading and block \
completion cover target-qualified links like `[[note#Head` and \
`[[note#^block`, same-destination links like `[[#Head`, and vault-wide \
searches like `[[##Head` and `[[^^block`. Candidate replacements own the \
missing closing delimiter when needed and report the final cursor offset.",
        )
        .after_help(
            "Examples:\n  bob capture-complete --cursor 1 -- '@'\n  bob capture-complete -c 19 -f json -- 'jot idea @notes#Id'\n  bob capture-complete -c 12 -b ~/bob -- 'Do work @Dev^new-id'\n  bob capture-complete -c 16 -b ~/bob -- 'Do work @Dev:foc'\n  bob capture-complete -c 5 -- '[[sas'\n\nContexts:\n  route, section, pomodoro_block_id, task, wikilink_note, wikilink_heading, wikilink_block",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
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

fn cursor_arg() -> Arg {
    Arg::new("cursor")
        .long("cursor")
        .short('c')
        .value_name("BYTE")
        .required(true)
        .value_parser(clap::value_parser!(usize))
        .help("UTF-8 byte offset of the cursor within TEXT")
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

fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Capture text; multiple args are joined with spaces")
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

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

/// Mirror `bob capture`'s and `bob capture-parse`'s convention: join every
/// TEXT argument with spaces, or read the complete piped stdin stream when
/// TEXT is omitted, minus exactly one trailing line terminator so a shell
/// pipe's closing newline never becomes part of the draft. Unlike
/// `capture-parse`, empty TEXT is not an error here: cursor 0 against an
/// empty draft is an ordinary interactive state that simply has no active
/// marker to complete.
fn raw_text_from_matches(matches: &ArgMatches) -> Result<String, String> {
    if let Some(values) = matches.get_many::<OsString>("text") {
        return Ok(values
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "));
    }

    if io::stdin().is_terminal() {
        return Ok(String::new());
    }

    let mut text = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|error| format!("read stdin: {error}"))?;
    if let Some(stripped) = text.strip_suffix("\r\n") {
        return Ok(stripped.to_string());
    }
    Ok(text.strip_suffix('\n').unwrap_or(&text).to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct Replacement {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RouteCandidate {
    replacement: String,
    route: String,
    label: String,
    kind: CaptureTargetKind,
    status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SectionCandidate {
    replacement: String,
    title: String,
    level: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TaskCandidate {
    replacement: String,
    #[serde(rename = "ref")]
    task_ref: String,
    block_id: String,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    depth: usize,
    child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum Candidates {
    Route(Vec<RouteCandidate>),
    Section(Vec<SectionCandidate>),
    Task(Vec<TaskCandidate>),
    WikilinkNote(Vec<WikilinkNoteCandidate>),
    WikilinkHeading(Vec<WikilinkHeadingCandidate>),
    WikilinkBlock(Vec<WikilinkBlockCandidate>),
}

impl Candidates {
    fn len(&self) -> usize {
        match self {
            Self::Route(items) => items.len(),
            Self::Section(items) => items.len(),
            Self::Task(items) => items.len(),
            Self::WikilinkNote(items) => items.len(),
            Self::WikilinkHeading(items) => items.len(),
            Self::WikilinkBlock(items) => items.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureCompleteResult {
    ok: bool,
    schema_version: u32,
    cursor: usize,
    replacement: Replacement,
    context: Option<CompletionContext>,
    candidates: Candidates,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl CaptureCompleteResult {
    fn empty(cursor: usize) -> Self {
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor,
            replacement: Replacement {
                start: cursor,
                end: cursor,
            },
            context: None,
            candidates: Candidates::Route(Vec::new()),
            warnings: Vec::new(),
        }
    }
}

fn build_result(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
) -> Result<CaptureCompleteResult, CompleteError> {
    let current_note_path = capture_language::parse_for_editor(raw_text)
        .route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if let Some(field) =
        capture_links::completion_field_at(raw_text, cursor, current_note_path)
    {
        let index = capture_links::NoteIndex::read(bob_dir)
            .map_err(CompleteError::io)?;
        let candidates = match field.context {
            CompletionContext::WikilinkNote => Candidates::WikilinkNote(
                capture_links::note_candidates(&field, &index),
            ),
            CompletionContext::WikilinkHeading => Candidates::WikilinkHeading(
                capture_links::heading_candidates(&field, &index),
            ),
            CompletionContext::WikilinkBlock => Candidates::WikilinkBlock(
                capture_links::block_candidates(&field, &index),
            ),
            CompletionContext::Route
            | CompletionContext::Section
            | CompletionContext::PomodoroBlockId
            | CompletionContext::Task => unreachable!("link field context"),
        };

        return Ok(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor,
            replacement: Replacement {
                start: field.replacement.0,
                end: field.replacement.1,
            },
            context: Some(field.context),
            candidates,
            warnings: index.warnings(),
        });
    }

    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(CaptureCompleteResult::empty(cursor));
    };

    let candidates = match field.context {
        CompletionContext::Route => route_candidates(bob_dir, &field.query)?,
        CompletionContext::Section => {
            let route = field.route.as_deref().expect("route resolved");
            section_candidates(bob_dir, route, &field.query)?
        }
        CompletionContext::PomodoroBlockId | CompletionContext::Task => {
            let route = field.route.as_deref().expect("route resolved");
            task_candidates(bob_dir, route, &field.query)?
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => {
            unreachable!("marker field context")
        }
    };

    Ok(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        cursor,
        replacement: Replacement {
            start: field.replacement.0,
            end: field.replacement.1,
        },
        context: Some(field.context),
        candidates,
        warnings: Vec::new(),
    })
}

fn route_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let report = capture_targets::scan_capture_targets(bob_dir);
    if !report.issues.is_empty() {
        return Err(CompleteError::io(report.issue_summary()));
    }

    let ranked = rank(report.targets, query, |target| target.route.as_str());
    Ok(Candidates::Route(
        ranked
            .into_iter()
            .map(|target| RouteCandidate {
                replacement: target.route.clone(),
                route: target.route,
                label: target.label,
                kind: target.kind,
                status: target.status,
            })
            .collect(),
    ))
}

fn section_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let sections = capture::non_tasks_section_headings(&contents);
    let ranked = rank(sections, query, |section| section.title.as_str());

    Ok(Candidates::Section(
        ranked
            .into_iter()
            .map(|section| SectionCandidate {
                replacement: section.title.clone(),
                title: section.title,
                level: section.level,
            })
            .collect(),
    ))
}

fn task_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let with_block_id: Vec<_> = scan
        .open_tasks()
        .filter(|task| task.block_id.is_some())
        .collect();
    let ranked = rank(with_block_id, query, |task| {
        task.block_id.as_deref().expect("filtered to Some")
    });

    Ok(Candidates::Task(
        ranked
            .into_iter()
            .map(|task| {
                let block_id = task.block_id.clone().expect("filtered to Some");
                TaskCandidate {
                    replacement: block_id.clone(),
                    task_ref: format!(
                        "{}:{}",
                        task.line_index + 1,
                        task.digest
                    ),
                    block_id,
                    status_symbol: task.status_symbol,
                    status_name: task.status_name.clone(),
                    status_type: capture_tasks::status_type_label(
                        task.status_type,
                    ),
                    text: task.description.clone(),
                    section: task.section.clone(),
                    depth: capture_tasks::indentation_depth(&task.indentation),
                    child_count: task.child_count,
                }
            })
            .collect(),
    ))
}

/// Read one routed note's contents; a missing note is not an error, exactly
/// like `capture-sections` and `capture-tasks`.
fn read_target(bob_dir: &Path, route: &str) -> Result<String, CompleteError> {
    let target = bob_dir.join(capture::route_label(route));
    match fs::read_to_string(&target) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(String::new())
        }
        Err(error) => Err(CompleteError::io(format!(
            "read target {}: {error}",
            target.display()
        ))),
    }
}

/// Case-insensitive candidate ranking: exact prefix matches before
/// substring matches, keeping each discovery source's stable order within
/// each group. A non-matching item is dropped. An empty query keeps every
/// item so a fresh `@` lists the whole discovery set.
fn rank<T>(items: Vec<T>, query: &str, key: impl Fn(&T) -> &str) -> Vec<T> {
    if query.is_empty() {
        return items;
    }

    let query = query.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    for item in items {
        let value = key(&item).to_lowercase();
        if value.starts_with(&query) {
            prefix_matches.push(item);
        } else if value.contains(&query) {
            substring_matches.push(item);
        }
    }

    prefix_matches.extend(substring_matches);
    prefix_matches
}

fn print_success(result: &CaptureCompleteResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureCompleteResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureCompleteResult,
    styler: &Styler,
) {
    let context_label = result.context.map(context_label).unwrap_or("none");
    println!(
        "Capture complete {} {}",
        styler.separator(),
        styler.cyan(context_label)
    );
    println!();
    println!(
        "  {}  {}-{}",
        styler.dim("replacement"),
        result.replacement.start,
        result.replacement.end
    );

    if result.candidates.len() == 0 {
        println!();
        println!("  No candidates found.");
        print_warnings(result, styler);
        println!();
        println!("0 candidates");
        return;
    }

    println!();
    println!("  Candidates");
    for line in candidate_lines(&result.candidates) {
        println!("    {} {}", styler.cyan(&line.0), styler.dim(&line.1));
    }
    print_warnings(result, styler);
    println!();
    println!("{} {}", result.candidates.len(), plural_candidates(result));
}

fn print_warnings(result: &CaptureCompleteResult, styler: &Styler) {
    if result.warnings.is_empty() {
        return;
    }
    println!();
    println!("  Warnings");
    for warning in &result.warnings {
        println!("    {}", styler.yellow(warning));
    }
}

fn plural_candidates(result: &CaptureCompleteResult) -> &'static str {
    if result.candidates.len() == 1 {
        "candidate"
    } else {
        "candidates"
    }
}

fn candidate_lines(candidates: &Candidates) -> Vec<(String, String)> {
    match candidates {
        Candidates::Route(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  {:?}", item.label, item.kind),
                )
            })
            .collect(),
        Candidates::Section(items) => items
            .iter()
            .map(|item| (item.replacement.clone(), format!("H{}", item.level)))
            .collect(),
        Candidates::Task(items) => items
            .iter()
            .map(|item| (item.replacement.clone(), item.text.clone()))
            .collect(),
        Candidates::WikilinkNote(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    item.alias.as_ref().map_or_else(
                        || item.path.clone(),
                        |alias| format!("{}  alias {alias}", item.path),
                    ),
                )
            })
            .collect(),
        Candidates::WikilinkHeading(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  H{}", item.path, item.level),
                )
            })
            .collect(),
        Candidates::WikilinkBlock(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    item.preview.as_ref().map_or_else(
                        || item.path.clone(),
                        |preview| format!("{}  {}", item.path, preview),
                    ),
                )
            })
            .collect(),
    }
}

fn context_label(context: CompletionContext) -> &'static str {
    match context {
        CompletionContext::Route => "route",
        CompletionContext::Section => "section",
        CompletionContext::PomodoroBlockId => "pomodoro_block_id",
        CompletionContext::Task => "task",
        CompletionContext::WikilinkNote => "wikilink_note",
        CompletionContext::WikilinkHeading => "wikilink_heading",
        CompletionContext::WikilinkBlock => "wikilink_block",
    }
}

fn success_json(result: &CaptureCompleteResult) -> String {
    serde_json::to_string(result).expect("serialize capture complete result")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompleteError {
    kind: CompleteErrorKind,
    message: String,
}

impl CompleteError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CompleteErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CompleteErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompleteErrorKind {
    Usage,
    Io,
}

impl CompleteErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

fn print_error(error: &CompleteError, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn result(
        bob_dir: &Path,
        raw: &str,
        cursor: usize,
    ) -> CaptureCompleteResult {
        build_result(bob_dir, raw, cursor).expect("build result")
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn empty_completion_has_no_context_and_a_zero_length_replacement() {
        let temp = TempDir::new("bob-cli-capture-complete-empty");
        let value = result(temp.path(), "buy milk", 4);
        assert_eq!(value.cursor, 4);
        assert_eq!(value.context, None);
        assert_eq!(value.replacement, Replacement { start: 4, end: 4 });
        assert_eq!(value.candidates.len(), 0);
    }

    #[test]
    fn route_completion_ranks_prefix_matches_before_substring_matches() {
        let temp = TempDir::new("bob-cli-capture-complete-routes");
        write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
        write_file(
            &temp.path().join("cash-flow.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(
            &temp.path().join("petty-cash.md"),
            "---\ntype: [[area]]\n---\n",
        );

        let value = result(temp.path(), "@ca", 3);
        assert_eq!(value.context, Some(CompletionContext::Route));
        assert_eq!(value.replacement, Replacement { start: 1, end: 3 });
        let Candidates::Route(routes) = &value.candidates else {
            panic!("expected route candidates");
        };
        let names: Vec<&str> =
            routes.iter().map(|route| route.route.as_str()).collect();
        assert_eq!(names, vec!["cash", "cash-flow", "petty-cash"]);
    }

    #[test]
    fn route_completion_lists_every_target_for_an_empty_query() {
        let temp = TempDir::new("bob-cli-capture-complete-routes-empty");
        let value = result(temp.path(), "@", 1);
        assert_eq!(value.context, Some(CompletionContext::Route));
        let Candidates::Route(routes) = &value.candidates else {
            panic!("expected route candidates");
        };
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, "mac_inbox");
        assert_eq!(routes[0].kind, CaptureTargetKind::Inbox);
    }

    #[test]
    fn section_completion_lists_headings_of_the_resolved_route() {
        let temp = TempDir::new("bob-cli-capture-complete-sections");
        write_file(
            &temp.path().join("notes.md"),
            "# Ideas\n## Ignored\n## Tasks\n### Inbox Ideas\n",
        );

        let value = result(temp.path(), "Idea @notes#Id", 14);
        assert_eq!(value.context, Some(CompletionContext::Section));
        let Candidates::Section(sections) = &value.candidates else {
            panic!("expected section candidates");
        };
        let titles: Vec<&str> = sections
            .iter()
            .map(|section| section.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Ideas", "Inbox Ideas"]);
    }

    #[test]
    fn section_completion_on_a_missing_note_is_an_empty_success() {
        let temp = TempDir::new("bob-cli-capture-complete-sections-missing");
        let value = result(temp.path(), "Idea @notes#", 12);
        assert_eq!(value.context, Some(CompletionContext::Section));
        assert_eq!(value.candidates.len(), 0);
    }

    #[test]
    fn pomodoro_block_id_completion_only_offers_tasks_with_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-complete-pomodoro");
        write_settings(temp.path());
        write_file(
            &temp.path().join("dev.md"),
            concat!(
                "- [ ] #task No block ID\n",
                "- [ ] #task Focus session ^focus-123\n",
                "- [ ] #task Other focus ^focus-999\n",
            ),
        );

        let value = result(temp.path(), "Do work @Dev:foc", 16);
        assert_eq!(value.context, Some(CompletionContext::PomodoroBlockId));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        let ids: Vec<&str> =
            tasks.iter().map(|task| task.block_id.as_str()).collect();
        assert_eq!(ids, vec!["focus-123", "focus-999"]);
    }

    #[test]
    fn sub_bullet_task_completion_reports_full_task_metadata() {
        let temp = TempDir::new("bob-cli-capture-complete-sub-bullet");
        write_settings(temp.path());
        write_file(
            &temp.path().join("cash.md"),
            "# Tasks\n- [*] #task Finish Google Exit Packet! ^goog-exit\n",
        );

        let value = result(temp.path(), "note @Cash+goog", 15);
        assert_eq!(value.context, Some(CompletionContext::Task));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.replacement, "goog-exit");
        assert_eq!(task.block_id, "goog-exit");
        assert_eq!(task.text, "Finish Google Exit Packet!");
        assert_eq!(task.section.as_deref(), Some("Tasks"));
        assert_eq!(task.status_symbol, '*');
        assert_eq!(task.status_type, "ON_HOLD");
        assert_eq!(task.child_count, 0);
    }

    #[test]
    fn task_block_id_completion_offers_routes_but_not_authored_ids() {
        let temp = TempDir::new("bob-cli-capture-complete-task-block-id");
        write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
        write_file(
            &temp.path().join("dev.md"),
            "# Tasks\n- [ ] #task Existing ^existing-id\n",
        );

        let route_side = result(temp.path(), "Do @ca^new-id", 6);
        assert_eq!(route_side.context, Some(CompletionContext::Route));
        let Candidates::Route(routes) = &route_side.candidates else {
            panic!("expected route candidates");
        };
        assert_eq!(routes[0].route, "cash");

        let id_side = result(temp.path(), "Do @dev^new-id", 14);
        assert_eq!(id_side.context, None);
        assert_eq!(id_side.candidates.len(), 0);
    }

    #[test]
    fn wikilink_note_completion_returns_alias_metadata_and_cursor_after() {
        let temp = TempDir::new("bob-cli-capture-complete-link-note");
        write_file(
            &temp.path().join("Artificial Intelligence.md"),
            "---\naliases: [AI]\n---\n",
        );

        let value = result(temp.path(), "[[AI", 4);
        assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
        assert_eq!(value.replacement, Replacement { start: 2, end: 4 });
        let Candidates::WikilinkNote(notes) = &value.candidates else {
            panic!("expected wikilink note candidates");
        };
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].replacement, "Artificial Intelligence|AI]]");
        assert_eq!(notes[0].cursor_after, 30);
        assert_eq!(notes[0].path, "Artificial Intelligence.md");
        assert_eq!(notes[0].alias.as_deref(), Some("AI"));
    }

    #[test]
    fn wikilink_completion_takes_precedence_over_marker_text_inside_link() {
        let temp = TempDir::new("bob-cli-capture-complete-link-precedence");
        write_file(&temp.path().join("Project Dev.md"), "");

        let value = result(temp.path(), "[[Project @d", 11);
        assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
        assert_eq!(value.replacement, Replacement { start: 2, end: 12 });
    }

    #[test]
    fn wikilink_same_note_heading_uses_capture_route_then_inbox_fallback() {
        let temp = TempDir::new("bob-cli-capture-complete-link-heading");
        write_file(&temp.path().join("sase.md"), "# Design\n");
        write_file(&temp.path().join("mac_inbox.md"), "# Inbox\n");

        let routed = result(temp.path(), "@sase task [[#De", 16);
        assert_eq!(routed.context, Some(CompletionContext::WikilinkHeading));
        let Candidates::WikilinkHeading(headings) = &routed.candidates else {
            panic!("expected heading candidates");
        };
        assert_eq!(headings[0].replacement, "Design]]");
        assert_eq!(headings[0].path, "sase.md");

        let fallback = result(temp.path(), "[[#In", 5);
        let Candidates::WikilinkHeading(headings) = &fallback.candidates else {
            panic!("expected heading candidates");
        };
        assert_eq!(headings[0].path, "mac_inbox.md");
    }

    #[test]
    fn wikilink_completion_surfaces_bounded_index_warnings() {
        let temp = TempDir::new("bob-cli-capture-complete-link-warnings");
        write_file(&temp.path().join("Good.md"), "");
        write_file(&temp.path().join("Bad.md"), "---\naliases: [\n---\n");

        let value = result(temp.path(), "[[G", 3);
        assert_eq!(value.context, Some(CompletionContext::WikilinkNote));
        assert_eq!(value.warnings.len(), 1);
        assert!(value.warnings[0].contains("parse aliases in Bad.md"));
    }

    #[test]
    fn json_shape_is_stable() {
        let value = serde_json::to_value(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor: 3,
            replacement: Replacement { start: 1, end: 3 },
            context: Some(CompletionContext::Route),
            candidates: Candidates::Route(vec![RouteCandidate {
                replacement: "cash".to_string(),
                route: "cash".to_string(),
                label: "cash.md".to_string(),
                kind: CaptureTargetKind::Area,
                status: None,
            }]),
            warnings: Vec::new(),
        })
        .expect("json");

        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["cursor"], 3);
        assert_eq!(value["replacement"]["start"], 1);
        assert_eq!(value["replacement"]["end"], 3);
        assert_eq!(value["context"], "route");
        assert_eq!(value["candidates"][0]["replacement"], "cash");
        assert_eq!(value["candidates"][0]["route"], "cash");
        assert_eq!(value["candidates"][0]["kind"], "area");
        assert!(value["candidates"][0]["status"].is_null());
    }

    #[test]
    fn empty_json_context_is_null() {
        let value = serde_json::to_value(CaptureCompleteResult::empty(4))
            .expect("json");
        assert!(value["context"].is_null());
        assert_eq!(value["candidates"], serde_json::json!([]));
    }

    #[test]
    fn human_output_is_plain_without_color() {
        let styler = Styler::plain();
        assert!(!styler.is_color());
        print_human_success_with_styler(
            &CaptureCompleteResult::empty(0),
            &styler,
        );
        print_human_success_with_styler(
            &CaptureCompleteResult {
                ok: true,
                schema_version: SCHEMA_VERSION,
                cursor: 3,
                replacement: Replacement { start: 1, end: 3 },
                context: Some(CompletionContext::Route),
                candidates: Candidates::Route(vec![RouteCandidate {
                    replacement: "cash".to_string(),
                    route: "cash".to_string(),
                    label: "cash.md".to_string(),
                    kind: CaptureTargetKind::Area,
                    status: None,
                }]),
                warnings: Vec::new(),
            },
            &styler,
        );
    }

    fn write_settings(root: &Path) {
        write_file(
            &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"}
                ],
                "customStatuses": [
                  {"symbol":"*","name":"Next","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!("create parent {}: {error}", parent.display())
            });
        }
        fs::write(path, contents).unwrap_or_else(|error| {
            panic!("write {}: {error}", path.display())
        });
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| {
                panic!("create temp dir {}: {error}", path.display())
            });
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }
}
