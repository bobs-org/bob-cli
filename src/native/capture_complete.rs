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
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    capture_targets::{self, CaptureTargetKind},
    capture_task_sections, capture_tasks, env as bob_env,
    note_tasks::{self, BlockIdLookup},
    pomodoro,
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
    let all_tasks = matches.get_flag("all-tasks");
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

    match build_result(&bob_dir, &raw_text, cursor, all_tasks) {
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
highlighting derived from that command. TEXT accepts the same \
blank-line-separated batch draft `bob capture` does; completion always scopes \
to the item and physical line the cursor is on, so only that item's first \
(parent) line offers a leading marker and a later column-zero or valid \
two-space nested authored line only completes its own trailing marker. A \
cursor on a blank separator row returns an empty success. The \
service decides whether completion applies at all: an unrecognized marker, \
a cursor in plain body text, a cursor on an authored line's indentation or \
bullet marker itself, an orphaned nested line, or a cursor on a token in the middle of a line all return a \
successful empty result rather than an error.\n\n\
Route completion covers a bare '@', a still-typing '@fragment', and the \
missing route portion of '@^...', '@+...', '@:...', and '@#...', plus the route \
component of a '@@', '@@fragment', or '@@route+...' declaration anywhere in the draft. Replacement \
ranges for that declaration exclude both '@' sigils and the '+'. Parent-task \
completion covers '@@route+fragment' the same way it covers '@route+fragment', \
including -a/--all-tasks missing-ID candidates. An inherited global route also \
becomes the current note for same-note wikilink heading/block completion unless \
that item overrides it. Route completion is backed by the same \
scan as `bob capture-targets`. Section completion covers '@route#prefix', \
backed by the same scan as `bob capture-sections`. Task-section completion \
covers '@route+id#prefix' and a bare '@route+id#', backed by the same \
scanner as `bob capture-task-sections`; replacement text is the section \
slug. Pomodoro-name completion covers '@route:id#prefix', a bare \
'@route:id#', and '@route:#prefix'; it is backed by `bob \
capture-pomodoros`, offers only open entries, collapses duplicate named \
slugs, and keeps nameable rows after named rows even when the query is \
nonempty. Nameable rows set requires_name and use an empty replacement that \
updated clients must not insert. When the query is a nonempty valid \
Pomodoro name that would not select an open exact or prefix match, and \
today's ledger can uniquely place a new future entry, the first candidate \
is a create action: creates_pomodoro is true, replacement is the canonical \
selector, name is the canonical visible name, and ref is omitted. Accepting \
that row only canonicalizes the marker; `bob capture` creates the named \
placeholder later. Exact or prefix open-name matches stay first and do not \
receive a create row. Empty queries stay the existing discovery list. A \
missing daily note, a missing Pomodoros section, and multiple open timed \
Pomodoros stay write-free warnings without a create row. Pomodoro block-ID \
completion covers '@route:prefix' and parent-task completion covers \
'@route+prefix', both backed by the same open-task scan as \
`bob capture-tasks` and, by default, only offer tasks that already carry a \
block ID. Pass -a/--all-tasks to include open tasks that still need an ID, \
but only in the '@route+' task context; Pomodoro '@route:' completion stays \
identified-only so older callers never receive action candidates they cannot \
handle. Missing-ID task candidates keep a placeholder replacement that must \
not be inserted, expose a nullable block_id, carry the route and stale-safe \
ref, and set requires_block_id. Task search matches block ID, description, \
section, and status name or symbol; identified tasks stay ahead of \
unidentified tasks, and prefix matches precede substring matches inside \
each group. The authored ID portion of '@route^block-id' has no \
completion source and returns an empty success. An empty block-ID component \
('@route+#') returns a successful empty task-section list; an unresolvable \
parent task returns a successful empty list plus one bounded warning. Other \
contexts still rank \
exact prefix matches before substring matches, case-insensitively, while \
keeping each discovery source's stable order. Task-section ranking uses \
slug-prefix matches first, then slug-substring matches, in document order \
inside each tier.\n\n\
When the cursor is inside an Obsidian wikilink, wikilink completion takes \
precedence over capture-marker completion. Note completion covers `[[note` \
and offers Markdown note paths, stems, and aliases. Heading and block \
completion cover target-qualified links like `[[note#Head` and \
`[[note#^block`, same-destination links like `[[#Head`, and vault-wide \
searches like `[[##Head` and `[[^^block`. Candidate replacements own the \
missing closing delimiter when needed and report the final cursor offset.",
        )
        .after_help(
            "Examples:\n  bob capture-complete --cursor 1 -- '@'\n  bob capture-complete -c 4 -- '@@fo'\n  bob capture-complete -c 20 -- 'Buy milk @@gro'\n  bob capture-complete -c 19 -f json -- 'jot idea @notes#Id'\n  bob capture-complete -c 12 -b ~/bob -- 'Do work @Dev^new-id'\n  bob capture-complete -c 16 -b ~/bob -- 'Do work @Dev:foc'\n  bob capture-complete -c 16 -b ~/bob -- 'note @foo+bar#'\n  bob capture-complete -a -c 6 -f json -- '@file+'\n  bob capture-complete -a -c 8 -f json -- '@@file+'\n  bob capture-complete -c 5 -- '[[sas'\n\nContexts:\n  route, section, pomodoro_block_id, pomodoro_name, task, task_section, wikilink_note, wikilink_heading, wikilink_block",
        )
        .disable_help_flag(true)
        .arg(all_tasks_arg())
        .arg(bob_dir_arg())
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(text_arg())
}

fn all_tasks_arg() -> Arg {
    Arg::new("all-tasks")
        .long("all-tasks")
        .short('a')
        .action(ArgAction::SetTrue)
        .help(
            "Include open tasks that still need a block ID (task context only)",
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
    block_id: Option<String>,
    route: String,
    requires_block_id: bool,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    depth: usize,
    child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TaskSectionCandidate {
    replacement: String,
    title: String,
    slug: String,
    route: String,
    block_id: Option<String>,
    text: String,
    line: usize,
    child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PomodoroNameCandidate {
    replacement: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pomodoro_ref: Option<capture_pomodoros::PomodoroRef>,
    name: Option<String>,
    requires_name: bool,
    #[serde(skip_serializing_if = "is_false")]
    creates_pomodoro: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    state: PomodoroState,
    status_symbol: char,
    time_range: Option<String>,
    placeholder: bool,
    is_current: bool,
    child_count: usize,
    match_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum Candidates {
    Route(Vec<RouteCandidate>),
    Section(Vec<SectionCandidate>),
    Task(Vec<TaskCandidate>),
    TaskSection(Vec<TaskSectionCandidate>),
    PomodoroName(Vec<PomodoroNameCandidate>),
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
            Self::TaskSection(items) => items.len(),
            Self::PomodoroName(items) => items.len(),
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
    all_tasks: bool,
) -> Result<CaptureCompleteResult, CompleteError> {
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
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
            | CompletionContext::PomodoroName
            | CompletionContext::Task
            | CompletionContext::TaskSection => {
                unreachable!("link field context")
            }
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

    let (candidates, warnings) = match field.context {
        CompletionContext::Route => {
            (route_candidates(bob_dir, &field.query)?, Vec::new())
        }
        CompletionContext::Section => {
            let route = field.route.as_deref().expect("route resolved");
            (
                section_candidates(bob_dir, route, &field.query)?,
                Vec::new(),
            )
        }
        CompletionContext::PomodoroBlockId => {
            let route = field.route.as_deref().expect("route resolved");
            (
                task_candidates(
                    bob_dir,
                    route,
                    &field.query,
                    false,
                    TaskSearch::BlockIdOnly,
                )?,
                Vec::new(),
            )
        }
        CompletionContext::Task => {
            let route = field.route.as_deref().expect("route resolved");
            (
                task_candidates(
                    bob_dir,
                    route,
                    &field.query,
                    all_tasks,
                    TaskSearch::MultiField,
                )?,
                Vec::new(),
            )
        }
        CompletionContext::TaskSection => {
            let route = field.route.as_deref().expect("route resolved");
            task_section_candidates(
                bob_dir,
                route,
                field.block_id.as_deref(),
                &field.query,
            )?
        }
        CompletionContext::PomodoroName => {
            pomodoro_name_candidates(bob_dir, &field.query)?
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
        warnings,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskSearch {
    BlockIdOnly,
    MultiField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Prefix,
    Substring,
}

fn task_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let ranked =
        rank_open_tasks(scan.open_tasks(), query, include_missing, search);

    Ok(Candidates::Task(
        ranked
            .into_iter()
            .map(|task| {
                let requires_block_id = task.block_id.is_none();
                TaskCandidate {
                    replacement: task.block_id.clone().unwrap_or_default(),
                    task_ref: task.task_ref(),
                    block_id: task.block_id.clone(),
                    route: route.to_string(),
                    requires_block_id,
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

fn rank_open_tasks<'a>(
    tasks: impl Iterator<Item = &'a note_tasks::NoteTask>,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    let mut identified = Vec::new();
    let mut unidentified = Vec::new();
    for task in tasks {
        if task.block_id.is_some() {
            identified.push(task);
        } else if include_missing {
            unidentified.push(task);
        }
    }

    let mut ranked = rank_task_group(identified, query, search);
    ranked.extend(rank_task_group(unidentified, query, search));
    ranked
}

fn rank_task_group<'a>(
    tasks: Vec<&'a note_tasks::NoteTask>,
    query: &str,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    if query.is_empty() {
        return tasks;
    }

    let query = query.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    for task in tasks {
        match task_match_kind(task, &query, search) {
            Some(MatchKind::Prefix) => prefix_matches.push(task),
            Some(MatchKind::Substring) => substring_matches.push(task),
            None => {}
        }
    }
    prefix_matches.extend(substring_matches);
    prefix_matches
}

fn task_match_kind(
    task: &note_tasks::NoteTask,
    query: &str,
    search: TaskSearch,
) -> Option<MatchKind> {
    let mut prefix = false;
    let mut substring = false;
    for field in task_search_fields(task, search) {
        let value = field.to_lowercase();
        if value.starts_with(query) {
            prefix = true;
        } else if value.contains(query) {
            substring = true;
        }
    }
    if prefix {
        Some(MatchKind::Prefix)
    } else if substring {
        Some(MatchKind::Substring)
    } else {
        None
    }
}

fn task_section_candidates(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let Some(block_id) = block_id.filter(|id| !id.is_empty()) else {
        return Ok((Candidates::TaskSection(Vec::new()), Vec::new()));
    };

    let target = bob_dir.join(capture::route_label(route));
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::MissingNote,
                )],
            ));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read target {}: {error}",
                target.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        BlockIdLookup::Missing => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::Missing {
                        suggestion: scan
                            .suggest_block_id(block_id)
                            .map(str::to_string),
                    },
                )],
            ));
        }
        BlockIdLookup::Duplicate(count) => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::Duplicate(count),
                )],
            ));
        }
        BlockIdLookup::NotATask { .. } => {
            return Ok((
                Candidates::TaskSection(Vec::new()),
                vec![unresolvable_parent_warning(
                    route,
                    block_id,
                    TaskSectionLookupFailure::NotATask,
                )],
            ));
        }
    };

    let parent_text = parent.description.clone();
    let parent_block_id = parent.block_id.clone();
    let ranked = rank(
        capture_task_sections::task_sections(&contents, parent),
        query,
        |section| section.slug.as_str(),
    );

    Ok((
        Candidates::TaskSection(
            ranked
                .into_iter()
                .map(|section| TaskSectionCandidate {
                    replacement: section.slug.clone(),
                    title: section.title,
                    slug: section.slug,
                    route: route.to_string(),
                    block_id: parent_block_id.clone(),
                    text: parent_text.clone(),
                    line: section.line,
                    child_count: section.child_count,
                })
                .collect(),
        ),
        Vec::new(),
    ))
}

fn pomodoro_name_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = pomodoro::day_file_for(bob_dir);
    pomodoro_name_candidates_at(&day_file, query)
}

fn pomodoro_name_candidates_at(
    day_file: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Candidates::PomodoroName(Vec::new()),
                vec![bounded_warning(format!(
                    "Bob daily note does not exist: {}",
                    day_file.display()
                ))],
            ));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let scan = capture_pomodoros::scan(&contents);
    let mut warnings = Vec::new();
    if !scan.has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }
    warnings.extend(scan.warnings.iter().cloned());
    let candidates = pomodoro_name_candidates_from_scan(&scan, query);

    Ok((Candidates::PomodoroName(candidates), warnings))
}

fn pomodoro_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    let mut candidates =
        pomodoro_name_candidates_from_entries(&scan.entries, query);
    if let Some(creation) = pomodoro_creation_candidate(scan, query) {
        insert_pomodoro_creation_candidate(&mut candidates, creation, query);
    }
    candidates
}

fn pomodoro_creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Option<PomodoroNameCandidate> {
    let name = capture_pomodoros::named_creation_name(scan, query)?;
    Some(PomodoroNameCandidate {
        replacement: capture_language::selector_slug(&name),
        pomodoro_ref: None,
        name: Some(name),
        requires_name: false,
        creates_pomodoro: true,
        line: None,
        state: PomodoroState::Open,
        status_symbol: ' ',
        time_range: None,
        placeholder: true,
        is_current: false,
        child_count: 0,
        match_count: 1,
    })
}

fn insert_pomodoro_creation_candidate(
    candidates: &mut Vec<PomodoroNameCandidate>,
    creation: PomodoroNameCandidate,
    query: &str,
) {
    let query = query.to_lowercase();
    let index = candidates
        .iter()
        .position(|candidate| {
            candidate.requires_name
                || !candidate.replacement.to_lowercase().starts_with(&query)
        })
        .unwrap_or(candidates.len());
    candidates.insert(index, creation);
}

fn pomodoro_name_candidates_from_entries(
    entries: &[PomodoroEntry],
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    let open_entries = entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .collect::<Vec<_>>();
    let mut seen_slugs = Vec::<&str>::new();
    let mut named = Vec::new();
    for entry in &open_entries {
        if !entry.selectable || seen_slugs.contains(&entry.slug.as_str()) {
            continue;
        }
        let match_count = open_entries
            .iter()
            .filter(|candidate| {
                candidate.selectable && candidate.slug == entry.slug
            })
            .count();
        seen_slugs.push(&entry.slug);
        named.push(pomodoro_name_candidate(entry, false, match_count));
    }

    let mut candidates =
        rank(named, query, |candidate| candidate.replacement.as_str());
    candidates.extend(
        open_entries
            .into_iter()
            .filter(|entry| !entry.selectable)
            .map(|entry| pomodoro_name_candidate(entry, true, 1)),
    );
    candidates
}

fn pomodoro_name_candidate(
    entry: &PomodoroEntry,
    requires_name: bool,
    match_count: usize,
) -> PomodoroNameCandidate {
    PomodoroNameCandidate {
        replacement: if requires_name {
            String::new()
        } else {
            entry.slug.clone()
        },
        pomodoro_ref: Some(entry.pomodoro_ref.clone()),
        name: entry.name.clone(),
        requires_name,
        creates_pomodoro: false,
        line: Some(entry.line),
        state: entry.state,
        status_symbol: entry.status_symbol,
        time_range: entry.time_range.clone(),
        placeholder: entry.placeholder,
        is_current: entry.is_current,
        child_count: entry.child_count,
        match_count,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn bounded_warning(message: String) -> String {
    const LIMIT: usize = 300;
    if message.chars().count() <= LIMIT {
        return message;
    }
    let mut truncated = message.chars().take(LIMIT - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

enum TaskSectionLookupFailure {
    MissingNote,
    Missing { suggestion: Option<String> },
    Duplicate(usize),
    NotATask,
}

/// One warning, no draft text, and no task description.
fn unresolvable_parent_warning(
    route: &str,
    block_id: &str,
    failure: TaskSectionLookupFailure,
) -> String {
    match failure {
        TaskSectionLookupFailure::MissingNote => {
            format!("note does not exist: {route}.md")
        }
        TaskSectionLookupFailure::Missing { suggestion } => {
            match suggestion {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}?"
                ),
                None => {
                    format!("no task with block ID ^{block_id} in {route}.md")
                }
            }
        }
        TaskSectionLookupFailure::Duplicate(count) => {
            format!(
                "block ID ^{block_id} appears {count} times in {route}.md"
            )
        }
        TaskSectionLookupFailure::NotATask => {
            format!("^{block_id} in {route}.md is not a task")
        }
    }
}

fn task_search_fields(
    task: &note_tasks::NoteTask,
    search: TaskSearch,
) -> Vec<String> {
    match search {
        TaskSearch::BlockIdOnly => task.block_id.iter().cloned().collect(),
        TaskSearch::MultiField => {
            let mut fields = Vec::new();
            if let Some(block_id) = &task.block_id {
                fields.push(block_id.clone());
            }
            fields.push(task.description.clone());
            if let Some(section) = &task.section {
                fields.push(section.clone());
            }
            fields.push(task.status_name.clone());
            fields.push(task.status_symbol.to_string());
            fields
        }
    }
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
            .map(|item| {
                let label = if item.requires_block_id {
                    "needs id".to_string()
                } else {
                    item.replacement.clone()
                };
                (label, item.text.clone())
            })
            .collect(),
        Candidates::TaskSection(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  {} items", item.title, item.child_count),
                )
            })
            .collect(),
        Candidates::PomodoroName(items) => items
            .iter()
            .map(|item| {
                let name =
                    item.name.clone().unwrap_or_else(|| "unnamed".to_string());
                let slug = if item.replacement.is_empty() {
                    "-"
                } else {
                    &item.replacement
                };
                let time = item.time_range.as_deref().unwrap_or("planned");
                let badges = pomodoro_name_badges(item).join(" ");
                let detail = if badges.is_empty() {
                    format!("{slug}  {time}")
                } else {
                    format!("{slug}  {time}  {badges}")
                };
                (name, detail)
            })
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

fn pomodoro_name_badges(item: &PomodoroNameCandidate) -> Vec<String> {
    let mut badges = Vec::new();
    if item.is_current {
        badges.push("current".to_string());
    }
    if item.match_count > 1 {
        badges.push(format!("{} matches", item.match_count));
    }
    if item.requires_name {
        badges.push("name it".to_string());
    }
    if item.creates_pomodoro {
        badges.push("create".to_string());
    }
    badges
}

fn context_label(context: CompletionContext) -> &'static str {
    match context {
        CompletionContext::Route => "route",
        CompletionContext::Section => "section",
        CompletionContext::PomodoroBlockId => "pomodoro_block_id",
        CompletionContext::PomodoroName => "pomodoro_name",
        CompletionContext::Task => "task",
        CompletionContext::TaskSection => "task_section",
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
        build_result(bob_dir, raw, cursor, false).expect("build result")
    }

    fn result_all(
        bob_dir: &Path,
        raw: &str,
        cursor: usize,
    ) -> CaptureCompleteResult {
        build_result(bob_dir, raw, cursor, true).expect("build result")
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
        let ids: Vec<&str> = tasks
            .iter()
            .map(|task| task.block_id.as_deref().expect("identified"))
            .collect();
        assert_eq!(ids, vec!["focus-123", "focus-999"]);
        assert!(tasks.iter().all(|task| !task.requires_block_id));
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
        assert_eq!(task.block_id.as_deref(), Some("goog-exit"));
        assert_eq!(task.route, "cash");
        assert!(!task.requires_block_id);
        assert_eq!(task.text, "Finish Google Exit Packet!");
        assert_eq!(task.section.as_deref(), Some("Tasks"));
        assert_eq!(task.status_symbol, '*');
        assert_eq!(task.status_type, "ON_HOLD");
        assert_eq!(task.child_count, 0);
    }

    #[test]
    fn task_section_completion_lists_ranked_slugs_for_the_parent_task() {
        let temp = TempDir::new("bob-cli-capture-complete-task-section");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            concat!(
                "# Tasks\n",
                "- [ ] #task Parent task ^bar\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing\n",
                "\t- FUTURE WORKFLOW\n",
                "\t- NOTES\n",
                "\t- FUTURE WORK\n",
            ),
        );

        let empty = result(temp.path(), "note @foo+bar#", 14);
        assert_eq!(empty.context, Some(CompletionContext::TaskSection));
        assert_eq!(empty.replacement, Replacement { start: 14, end: 14 });
        let Candidates::TaskSection(all) = &empty.candidates else {
            panic!("expected task section candidates");
        };
        let titles: Vec<&str> =
            all.iter().map(|section| section.title.as_str()).collect();
        assert_eq!(
            titles,
            ["REQUIREMENTS", "FUTURE WORKFLOW", "NOTES", "FUTURE WORK"]
        );
        assert_eq!(all[0].replacement, "requirements");
        assert_eq!(all[0].slug, "requirements");
        assert_eq!(all[0].route, "foo");
        assert_eq!(all[0].block_id.as_deref(), Some("bar"));
        assert_eq!(all[0].text, "Parent task");
        assert_eq!(all[0].line, 3);
        assert_eq!(all[0].child_count, 1);
        assert_eq!(all[3].replacement, "future-work");
        assert_eq!(all[3].child_count, 0);
        assert!(empty.warnings.is_empty());

        let prefix = result(temp.path(), "note @foo+bar#future", 20);
        let Candidates::TaskSection(prefixed) = &prefix.candidates else {
            panic!("expected task section candidates");
        };
        let prefixed_titles: Vec<&str> = prefixed
            .iter()
            .map(|section| section.title.as_str())
            .collect();
        assert_eq!(prefixed_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);

        let exact = result(temp.path(), "note @foo+bar#future-work", 25);
        let Candidates::TaskSection(exact_hits) = &exact.candidates else {
            panic!("expected task section candidates");
        };
        let exact_titles: Vec<&str> = exact_hits
            .iter()
            .map(|section| section.title.as_str())
            .collect();
        assert_eq!(exact_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);
        let future_work = exact_hits
            .iter()
            .find(|section| section.title == "FUTURE WORK")
            .expect("FUTURE WORK");
        assert_eq!(future_work.replacement, "future-work");
        assert_eq!(future_work.slug, "future-work");

        let substring = result(temp.path(), "note @foo+bar#work", 18);
        let Candidates::TaskSection(subs) = &substring.candidates else {
            panic!("expected task section candidates");
        };
        let sub_titles: Vec<&str> =
            subs.iter().map(|section| section.title.as_str()).collect();
        assert_eq!(sub_titles, ["FUTURE WORKFLOW", "FUTURE WORK"]);
    }

    #[test]
    fn three_component_marker_keeps_route_and_task_contexts() {
        let temp = TempDir::new("bob-cli-capture-complete-three-component");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            concat!(
                "---\ntype: [[area]]\n---\n",
                "- [ ] #task Parent ^bar\n",
                "\t- REQUIREMENTS\n",
            ),
        );

        let raw = "note @foo+bar#req";
        let at = raw.find('@').expect("at");
        let plus = raw.find('+').expect("plus");
        let hash = raw.find('#').expect("hash");

        let route = result(temp.path(), raw, at + 3);
        assert_eq!(route.context, Some(CompletionContext::Route));
        let Candidates::Route(routes) = &route.candidates else {
            panic!("expected route candidates");
        };
        assert!(
            routes.iter().any(|candidate| candidate.route == "foo"),
            "{routes:?}"
        );

        let task = result(temp.path(), raw, plus + 2);
        assert_eq!(task.context, Some(CompletionContext::Task));
        let Candidates::Task(tasks) = &task.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks[0].block_id.as_deref(), Some("bar"));

        let section = result(temp.path(), raw, hash + 2);
        assert_eq!(section.context, Some(CompletionContext::TaskSection));
        let Candidates::TaskSection(sections) = &section.candidates else {
            panic!("expected task section candidates");
        };
        assert_eq!(sections[0].replacement, "requirements");
        assert_eq!(sections[0].title, "REQUIREMENTS");
    }

    #[test]
    fn task_section_completion_empty_block_id_is_an_empty_success() {
        let temp = TempDir::new("bob-cli-capture-complete-task-section-empty");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            "- [ ] #task Parent ^bar\n\t- REQUIREMENTS\n",
        );

        let value = result(temp.path(), "note @foo+#", 11);
        assert_eq!(value.context, Some(CompletionContext::TaskSection));
        assert_eq!(value.candidates.len(), 0);
        assert!(value.warnings.is_empty());
    }

    #[test]
    fn task_section_completion_warns_once_for_an_unresolvable_parent() {
        let temp =
            TempDir::new("bob-cli-capture-complete-task-section-warning");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            concat!(
                "Plain heading ^plain-id\n",
                "- [ ] #task Ready ^ready-id\n",
                "- [ ] #task Dup ^dup-id\n",
                "- [ ] #task Also dup ^dup-id\n",
            ),
        );

        let missing = result(temp.path(), "note @foo+missing#", 18);
        assert_eq!(missing.context, Some(CompletionContext::TaskSection));
        assert_eq!(missing.candidates.len(), 0);
        assert_eq!(missing.warnings.len(), 1);
        assert_eq!(
            missing.warnings[0],
            "no task with block ID ^missing in foo.md"
        );
        assert!(!missing.warnings[0].contains("note @foo"));

        let close = result(temp.path(), "note @foo+ready-i#", 18);
        assert_eq!(close.warnings.len(), 1);
        assert!(
            close.warnings[0].contains("did you mean ^ready-id"),
            "{}",
            close.warnings[0]
        );

        let duplicate = result(temp.path(), "note @foo+dup-id#", 17);
        assert_eq!(duplicate.warnings.len(), 1);
        assert_eq!(
            duplicate.warnings[0],
            "block ID ^dup-id appears 2 times in foo.md"
        );

        let not_a_task = result(temp.path(), "note @foo+plain-id#", 19);
        assert_eq!(not_a_task.warnings.len(), 1);
        assert_eq!(not_a_task.warnings[0], "^plain-id in foo.md is not a task");
        assert!(!not_a_task.warnings[0].contains("Plain heading"));

        let missing_note = result(temp.path(), "note @absent+bar#", 17);
        assert_eq!(missing_note.warnings.len(), 1);
        assert_eq!(missing_note.warnings[0], "note does not exist: absent.md");
    }

    #[test]
    fn pomodoro_name_completion_lists_named_then_nameable_rows() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "\t- [[dev#^focus]]\n",
            "- [ ] () — BUGS\n",
            "- [ ] () — MEMORY\n",
            "- [ ] ()\n",
            "- [ ] () — SNAKE_CASE\n",
            "- [x] () — DONE\n",
        ));

        let candidates =
            pomodoro_name_candidates_from_entries(&scan.entries, "");
        let rows = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.replacement.as_str(),
                    candidate.name.as_deref(),
                    candidate.requires_name,
                    candidate.creates_pomodoro,
                    candidate.line,
                    candidate.is_current,
                    candidate.child_count,
                    candidate.match_count,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("memory", Some("MEMORY"), false, false, Some(2), true, 1, 2),
                ("bugs", Some("BUGS"), false, false, Some(4), false, 0, 1),
                ("", None, true, false, Some(6), false, 0, 1),
                ("", Some("SNAKE_CASE"), true, false, Some(7), false, 0, 1),
            ]
        );
        assert_eq!(candidates[0].time_range.as_deref(), Some("0900-0930"));
        assert!(!candidates[0].placeholder);
        assert!(candidates[2].placeholder);
    }

    #[test]
    fn pomodoro_name_completion_keeps_nameable_rows_for_a_query() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — MEMORY\n",
            "- [ ] () — BUGS\n",
            "- [ ] ()\n",
            "- [ ] () — SNAKE_CASE\n",
            "- [x] () — BUGS DONE\n",
        ));

        let candidates =
            pomodoro_name_candidates_from_entries(&scan.entries, "bu");
        let rows = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.replacement.as_str(),
                    candidate.name.as_deref(),
                    candidate.requires_name,
                    candidate.creates_pomodoro,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("bugs", Some("BUGS"), false, false),
                ("", None, true, false),
                ("", Some("SNAKE_CASE"), true, false),
            ]
        );
    }

    #[test]
    fn pomodoro_name_completion_works_without_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-complete-pomodoro-name");
        write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
        let day_file = temp.path().join("2026/20260828.md");
        write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n- [ ] ()\n");

        let raw = "note @dev:#bu";
        let value = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });

        assert_eq!(value.context, Some(CompletionContext::PomodoroName));
        assert_eq!(
            value.replacement,
            Replacement {
                start: raw.find('#').expect("hash") + 1,
                end: raw.len(),
            }
        );
        let Candidates::PomodoroName(candidates) = &value.candidates else {
            panic!("expected Pomodoro-name candidates");
        };
        assert_eq!(candidates[0].replacement, "bugs");
        assert_eq!(candidates[0].name.as_deref(), Some("BUGS"));
        assert!(!candidates[0].requires_name);
        assert!(!candidates[0].creates_pomodoro);
        assert!(candidates[1].requires_name);
        assert!(!candidates[1].creates_pomodoro);
    }

    #[test]
    fn pomodoro_name_completion_offers_creation_before_substring_and_nameable_rows(
    ) {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] () — NETWORK\n",
            "- [ ] ()\n",
            "- [x] () — BUGS\n",
        ));

        let novel = pomodoro_name_candidates_from_scan(&scan, "future");
        assert_eq!(novel[0].replacement, "future");
        assert_eq!(novel[0].name.as_deref(), Some("FUTURE"));
        assert!(novel[0].creates_pomodoro);
        assert!(!novel[0].requires_name);
        assert!(novel[0].pomodoro_ref.is_none());
        assert!(novel[0].line.is_none());
        assert!(novel[0].placeholder);
        assert_eq!(novel[0].child_count, 0);
        assert!(novel.iter().skip(1).any(|row| row.requires_name));
        assert!(novel.iter().skip(1).all(|row| !row.creates_pomodoro));

        let completed_only = pomodoro_name_candidates_from_scan(&scan, "bugs");
        assert_eq!(completed_only[0].replacement, "bugs");
        assert_eq!(completed_only[0].name.as_deref(), Some("BUGS"));
        assert!(completed_only[0].creates_pomodoro);
        assert!(completed_only[0].pomodoro_ref.is_none());

        let substring_only = pomodoro_name_candidates_from_scan(&scan, "work");
        assert_eq!(substring_only[0].replacement, "work");
        assert_eq!(substring_only[0].name.as_deref(), Some("WORK"));
        assert!(substring_only[0].creates_pomodoro);
        assert_eq!(substring_only[1].replacement, "network");
        assert!(!substring_only[1].creates_pomodoro);
        assert!(substring_only[2].requires_name);
    }

    #[test]
    fn pomodoro_name_completion_suppresses_creation_for_open_name_matches() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] () — BUGS\n",
            "- [ ] ()\n",
        ));

        for query in ["memory", "mem", "MEMORY"] {
            let candidates = pomodoro_name_candidates_from_scan(&scan, query);
            assert!(
                candidates.iter().all(|row| !row.creates_pomodoro),
                "{query}: {candidates:?}"
            );
            assert_eq!(candidates[0].replacement, "memory");
        }
    }

    #[test]
    fn pomodoro_name_completion_skips_creation_for_empty_or_invalid_queries() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — MEMORY\n",
            "- [ ] ()\n",
        ));

        let empty = pomodoro_name_candidates_from_scan(&scan, "");
        assert!(!empty.is_empty());
        assert!(empty.iter().all(|row| !row.creates_pomodoro));

        let invalid = pomodoro_name_candidates_from_scan(&scan, "bad_id");
        assert!(invalid.iter().all(|row| !row.creates_pomodoro));
        assert!(invalid.iter().any(|row| row.requires_name));
    }

    #[test]
    fn pomodoro_name_completion_skips_creation_when_the_ledger_cannot_place_it()
    {
        let missing_section = capture_pomodoros::scan("# Day\n");
        assert!(!missing_section.has_section);
        let skipped =
            pomodoro_name_candidates_from_scan(&missing_section, "future");
        assert!(skipped.is_empty());

        let ambiguous = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] (1000-1030) — BUGS\n",
            "- [ ] ()\n",
        ));
        let candidates =
            pomodoro_name_candidates_from_scan(&ambiguous, "future");
        assert!(candidates.iter().all(|row| !row.creates_pomodoro));
        assert!(candidates.iter().any(|row| row.requires_name));

        let named_still_wins =
            pomodoro_name_candidates_from_scan(&ambiguous, "mem");
        assert_eq!(named_still_wins[0].replacement, "memory");
        assert!(!named_still_wins[0].creates_pomodoro);
    }

    #[test]
    fn pomodoro_name_completion_missing_daily_note_warns() {
        let temp =
            TempDir::new("bob-cli-capture-complete-pomodoro-name-missing");
        let missing_day = temp.path().join("2026/20260828.md");

        let (candidates, warnings) =
            pomodoro_name_candidates_at(&missing_day, "")
                .expect("warning success");

        assert_eq!(candidates.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("does not exist"));

        let sectionless = temp.path().join("2026/20260829.md");
        write_file(&sectionless, "# Day\n");
        let (candidates, warnings) =
            pomodoro_name_candidates_at(&sectionless, "")
                .expect("warning success");

        assert_eq!(candidates.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no Pomodoros section"));
    }

    #[test]
    fn default_task_completion_stays_identified_only() {
        let temp = TempDir::new("bob-cli-capture-complete-identified-only");
        write_settings(temp.path());
        write_file(
            &temp.path().join("file.md"),
            concat!(
                "# Tasks\n",
                "- [ ] #task No block ID\n",
                "- [ ] #task Ready one ^ready-one\n",
                "- [x] #task Done task\n",
                "- [*] #task Ready two ^ready-two\n",
            ),
        );

        let value = result(temp.path(), "note @file+", 11);
        assert_eq!(value.context, Some(CompletionContext::Task));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        let ids: Vec<Option<&str>> =
            tasks.iter().map(|task| task.block_id.as_deref()).collect();
        assert_eq!(ids, vec![Some("ready-one"), Some("ready-two")]);
        assert!(tasks.iter().all(|task| !task.requires_block_id));
    }

    #[test]
    fn all_tasks_lists_identified_tasks_before_unidentified_tasks() {
        let temp = TempDir::new("bob-cli-capture-complete-all-tasks");
        write_settings(temp.path());
        write_file(
            &temp.path().join("file.md"),
            concat!(
                "# Inbox\n",
                "- [ ] #task First missing\n",
                "- [ ] #task Ready one ^ready-one\n",
                "- [x] #task Done missing\n",
                "- [*] #task Ready two ^ready-two\n",
                "- [/] #task Second missing\n",
            ),
        );

        let value = result_all(temp.path(), "note @file+", 11);
        assert_eq!(value.context, Some(CompletionContext::Task));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        let rows: Vec<(Option<&str>, &str, bool)> = tasks
            .iter()
            .map(|task| {
                (
                    task.block_id.as_deref(),
                    task.text.as_str(),
                    task.requires_block_id,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (Some("ready-one"), "Ready one", false),
                (Some("ready-two"), "Ready two", false),
                (None, "First missing", true),
                (None, "Second missing", true),
            ]
        );
        assert_eq!(tasks[2].replacement, "");
        assert_eq!(tasks[2].route, "file");
        assert!(!tasks[2].task_ref.is_empty());
    }

    #[test]
    fn all_tasks_search_keeps_identified_groups_ahead_of_unidentified() {
        let temp = TempDir::new("bob-cli-capture-complete-all-search");
        write_settings(temp.path());
        write_file(
            &temp.path().join("file.md"),
            concat!(
                "# Planning\n",
                "- [ ] #task Draft report\n",
                "- [ ] #task Ready alpha ^alpha-id\n",
                "# Review\n",
                "- [*] #task Planning notes ^later-id\n",
                "- [/] #task Alpha follow-up\n",
            ),
        );

        let by_id_and_text = result_all(temp.path(), "note @file+alpha", 16);
        let Candidates::Task(tasks) = &by_id_and_text.candidates else {
            panic!("expected task candidates");
        };
        let texts: Vec<&str> =
            tasks.iter().map(|task| task.text.as_str()).collect();
        assert_eq!(texts, vec!["Ready alpha", "Alpha follow-up"]);
        assert!(!tasks[0].requires_block_id);
        assert!(tasks[1].requires_block_id);

        let by_status = result_all(temp.path(), "note @file+Next", 15);
        let Candidates::Task(tasks) = &by_status.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "Planning notes");
        assert_eq!(tasks[0].status_name, "Next");

        let by_section = result_all(temp.path(), "note @file+rev", 14);
        let Candidates::Task(tasks) = &by_section.candidates else {
            panic!("expected task candidates");
        };
        let texts: Vec<&str> =
            tasks.iter().map(|task| task.text.as_str()).collect();
        assert_eq!(texts, vec!["Planning notes", "Alpha follow-up"]);
    }

    #[test]
    fn all_tasks_does_not_change_pomodoro_completion() {
        let temp = TempDir::new("bob-cli-capture-complete-all-pomodoro");
        write_settings(temp.path());
        write_file(
            &temp.path().join("dev.md"),
            concat!(
                "- [ ] #task No block ID\n",
                "- [ ] #task Focus session ^focus-123\n",
            ),
        );

        let value = result_all(temp.path(), "Do work @Dev:foc", 16);
        assert_eq!(value.context, Some(CompletionContext::PomodoroBlockId));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].block_id.as_deref(), Some("focus-123"));
        assert!(!tasks[0].requires_block_id);
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
    fn wikilink_same_note_heading_uses_the_cursor_item_route() {
        let temp = TempDir::new("bob-cli-capture-complete-batch-link-heading");
        write_file(&temp.path().join("work.md"), "# Work\n");
        write_file(&temp.path().join("sase.md"), "# Design\n");

        let draft = "@work first\n\n@sase second [[#De";
        let value = result(temp.path(), draft, draft.len());
        assert_eq!(value.context, Some(CompletionContext::WikilinkHeading));
        let Candidates::WikilinkHeading(headings) = &value.candidates else {
            panic!("expected heading candidates");
        };
        assert_eq!(headings[0].replacement, "Design]]");
        assert_eq!(headings[0].path, "sase.md");
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
        let scan = capture_pomodoros::scan(
            "## Pomodoros\n- [ ] (1205-1230) — MEMORY\n",
        );
        let pomodoro_name =
            pomodoro_name_candidates_from_entries(&scan.entries, "mem")
                .remove(0);
        let pomodoro_json = serde_json::to_value(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor: 10,
            replacement: Replacement { start: 9, end: 10 },
            context: Some(CompletionContext::PomodoroName),
            candidates: Candidates::PomodoroName(vec![pomodoro_name]),
            warnings: Vec::new(),
        })
        .expect("pomodoro json");

        assert_eq!(pomodoro_json["context"], "pomodoro_name");
        assert_eq!(pomodoro_json["candidates"][0]["replacement"], "memory");
        assert_eq!(pomodoro_json["candidates"][0]["name"], "MEMORY");
        assert_eq!(pomodoro_json["candidates"][0]["requires_name"], false);
        assert!(pomodoro_json["candidates"][0]
            .get("creates_pomodoro")
            .is_none());
        assert_eq!(pomodoro_json["candidates"][0]["line"], 2);
        assert_eq!(pomodoro_json["candidates"][0]["state"], "open");
        assert_eq!(pomodoro_json["candidates"][0]["status_symbol"], " ");
        assert_eq!(pomodoro_json["candidates"][0]["time_range"], "1205-1230");
        assert_eq!(pomodoro_json["candidates"][0]["placeholder"], false);
        assert_eq!(pomodoro_json["candidates"][0]["is_current"], true);
        assert_eq!(pomodoro_json["candidates"][0]["child_count"], 0);
        assert_eq!(pomodoro_json["candidates"][0]["match_count"], 1);
        assert!(pomodoro_json["candidates"][0]["ref"]
            .as_str()
            .expect("ref")
            .contains(':'));

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

    #[test]
    fn pomodoro_name_human_rows_include_time_and_badges() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] () — MEMORY\n",
            "- [ ] ()\n",
        ));
        let candidates = Candidates::PomodoroName(
            pomodoro_name_candidates_from_entries(&scan.entries, ""),
        );

        let lines = candidate_lines(&candidates);

        assert_eq!(lines[0].0, "MEMORY");
        assert_eq!(lines[0].1, "memory  0900-0930  current 2 matches");
        assert_eq!(lines[1].0, "unnamed");
        assert_eq!(lines[1].1, "-  planned  name it");
    }

    #[test]
    fn pomodoro_name_human_rows_badge_creation() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] ()\n",
        ));
        let candidates = Candidates::PomodoroName(
            pomodoro_name_candidates_from_scan(&scan, "future"),
        );
        let lines = candidate_lines(&candidates);

        assert_eq!(lines[0].0, "FUTURE");
        assert_eq!(lines[0].1, "future  planned  create");
        assert!(lines.iter().any(|line| line.1.contains("name it")));
    }

    #[test]
    fn pomodoro_creation_json_omits_ref_and_keeps_schema_version() {
        let scan = capture_pomodoros::scan(
            "## Pomodoros\n- [ ] (1205-1230) — MEMORY\n- [ ] ()\n",
        );
        let creation = pomodoro_name_candidates_from_scan(&scan, "future")
            .into_iter()
            .find(|row| row.creates_pomodoro)
            .expect("creation row");
        let json = serde_json::to_value(CaptureCompleteResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor: 10,
            replacement: Replacement { start: 9, end: 10 },
            context: Some(CompletionContext::PomodoroName),
            candidates: Candidates::PomodoroName(vec![creation]),
            warnings: Vec::new(),
        })
        .expect("creation json");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["candidates"][0]["replacement"], "future");
        assert_eq!(json["candidates"][0]["name"], "FUTURE");
        assert_eq!(json["candidates"][0]["creates_pomodoro"], true);
        assert_eq!(json["candidates"][0]["requires_name"], false);
        assert_eq!(json["candidates"][0]["placeholder"], true);
        assert!(json["candidates"][0].get("ref").is_none());
        assert!(json["candidates"][0].get("line").is_none());
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

    fn with_env<T>(
        key: &str,
        value: impl Into<OsString>,
        f: impl FnOnce() -> T,
    ) -> T {
        let old = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value.into());
        }
        let result = f();
        unsafe {
            match old {
                Some(old) => std::env::set_var(key, old),
                None => std::env::remove_var(key),
            }
        }
        result
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
