//! Task-section scanner and read-only `bob capture-task-sections` CLI.
//!
//! This module is the single owner of the title predicate, checkbox detection,
//! slug, direct-child enumeration, selector matching, and insertion geometry.

use std::{ffi::OsString, fs, io, iter, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture::{
        self, dominant_indent_unit, first_child_indentation,
        first_direct_managed_log_start, leading_spaces_or_tabs_len,
        leading_whitespace, line_spans, list_item_body,
        nearest_shallower_list_item_parent, parse_managed_task_log_marker,
        LineSpan,
    },
    capture_language, env as bob_env,
    note_tasks::{self, BlockIdLookup, NoteTask, RefLookup, TaskRef},
    style::Styler,
};

/// A direct-child ALL-CAPS section bullet under a resolved task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSection {
    pub(crate) title: String,
    pub(crate) slug: String,
    /// One-based line number of the section bullet.
    pub(crate) line: usize,
    pub(crate) indentation: String,
    pub(crate) block_end: usize,
    pub(crate) child_count: usize,
}

impl TaskSection {
    pub(crate) fn line_index(&self) -> usize {
        self.line - 1
    }
}

/// Where a capture block lands under a selected section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSectionInsertion {
    pub(crate) offset: usize,
    pub(crate) indentation: String,
}

/// Canonical whitespace-free slug: trim, collapse internal whitespace to one
/// space, ASCII-lowercase, then replace each remaining space with `-`.
pub(crate) fn slug(text: &str) -> String {
    let mut result = String::new();
    for word in text.split_whitespace() {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&word.to_ascii_lowercase());
    }
    result
}

/// Enumerate qualifying section bullets that are direct children of `task`.
pub(crate) fn task_sections(
    contents: &str,
    task: &NoteTask,
) -> Vec<TaskSection> {
    let lines = line_spans(contents);
    let children =
        direct_child_list_indexes(&lines, task.line_index, task.block_end);
    let mut sections = Vec::new();
    for (index, &line_index) in children.iter().enumerate() {
        let line = lines[line_index];
        if parse_managed_task_log_marker(line.text).is_some() {
            continue;
        }
        let Some(body) = list_item_body(line.text) else {
            continue;
        };
        let Some(title) = parse_task_section_title(body) else {
            continue;
        };
        let block_end = children
            .get(index + 1)
            .map(|&next| line_start(&lines, next))
            .unwrap_or(task.block_end);
        let child_count =
            direct_child_list_indexes(&lines, line_index, block_end).len();
        sections.push(TaskSection {
            title: title.to_string(),
            slug: slug(title),
            line: line_index + 1,
            indentation: leading_whitespace(line.text).to_string(),
            block_end,
            child_count,
        });
    }
    sections
}

/// Whole-slug match in document order, else the first slug-prefix match.
pub(crate) fn select_section<'a>(
    sections: &'a [TaskSection],
    selector: &str,
) -> Option<&'a TaskSection> {
    let needle = slug(selector);
    if needle.is_empty() {
        return None;
    }
    sections
        .iter()
        .find(|section| section.slug == needle)
        .or_else(|| {
            sections
                .iter()
                .find(|section| section.slug.starts_with(&needle))
        })
}

/// Whole-title match in document order, compared case-insensitively.
///
/// This is the picker path (`--task-section TITLE`): hyphenated slugs are
/// not rewritten, so `future-work` does not match `FUTURE WORK`.
pub(crate) fn select_section_exact<'a>(
    sections: &'a [TaskSection],
    title: &str,
) -> Option<&'a TaskSection> {
    sections
        .iter()
        .find(|section| section.title.eq_ignore_ascii_case(title))
}

/// Dispatch slug/prefix matching or exact title matching.
pub(crate) fn match_section<'a>(
    sections: &'a [TaskSection],
    selector: &str,
    exact: bool,
) -> Option<&'a TaskSection> {
    if exact {
        select_section_exact(sections, selector)
    } else {
        select_section(sections, selector)
    }
}

/// Unique nearby section for a failed selector, or `None` when several
/// candidates are equally close. Slug matching compares slugs; exact
/// matching compares lowercased titles.
pub(crate) fn suggest_section<'a>(
    sections: &'a [TaskSection],
    selector: &str,
    exact: bool,
) -> Option<&'a TaskSection> {
    let requested = if exact {
        selector.to_ascii_lowercase()
    } else {
        slug(selector)
    };
    if requested.is_empty() {
        return None;
    }
    let key = |section: &TaskSection| {
        if exact {
            section.title.to_ascii_lowercase()
        } else {
            section.slug.clone()
        }
    };
    if sections.iter().any(|section| key(section) == requested) {
        return None;
    }
    let mut close = sections.iter().filter(|section| {
        super::note_tasks::bounded_levenshtein(&requested, &key(section), 2)
            .is_some()
    });
    match (close.next(), close.next()) {
        (Some(section), None) => Some(section),
        _ => None,
    }
}

/// Insertion offset and indent for a capture nested under `section`.
pub(crate) fn section_insertion(
    contents: &str,
    section: &TaskSection,
) -> TaskSectionInsertion {
    let lines = line_spans(contents);
    let indentation = first_child_indentation(
        &lines,
        section.line_index(),
        section.block_end,
        &section.indentation,
    )
    .or_else(|| {
        dominant_indent_unit(&lines)
            .map(|unit| format!("{}{}", section.indentation, unit))
    })
    .unwrap_or_else(|| format!("{}\t", section.indentation));
    let offset = first_direct_managed_log_start(
        &lines,
        section.line_index(),
        section.block_end,
    )
    .unwrap_or(section.block_end);
    TaskSectionInsertion {
        offset,
        indentation,
    }
}

/// Title of a list-item body when it is a task section, else `None`.
///
/// Capture does **not** require a nested list item under the bullet. The
/// plugin's Ctrl+Shift+Alt+N conversion does, because a childless bullet would
/// seed an empty `##` heading. Requiring that lookahead would reject a freshly
/// authored, still-empty `REQUIREMENTS` bullet — the moment this feature is
/// most useful. `child_count` lets a picker badge emptiness instead.
fn parse_task_section_title(body: &str) -> Option<&str> {
    if has_task_checkbox(body) {
        return None;
    }
    let title = body.trim();
    is_section_title(title).then_some(title)
}

fn is_section_title(title: &str) -> bool {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_section_title_first(first) {
        return false;
    }
    let mut has_letter = first.is_ascii_uppercase();
    for character in chars {
        if !is_section_title_rest(character) {
            return false;
        }
        has_letter |= character.is_ascii_uppercase();
    }
    has_letter
}

fn is_section_title_first(character: char) -> bool {
    character.is_ascii_uppercase() || character.is_ascii_digit()
}

fn is_section_title_rest(character: char) -> bool {
    is_section_title_first(character)
        || matches!(
            character,
            ' ' | '\t' | '&' | '\'' | '(' | ')' | ',' | '.' | '/' | '-'
        )
}

/// Plugin `PROJECT_CHILD_LIST_ITEM_RE` checkbox: `[x]` plus following
/// whitespace, where `x` is a single character other than `]` or newline.
fn has_task_checkbox(body: &str) -> bool {
    let mut chars = body.chars();
    if chars.next() != Some('[') {
        return false;
    }
    let Some(status) = chars.next() else {
        return false;
    };
    if status == ']' || status == '\n' {
        return false;
    }
    if chars.next() != Some(']') {
        return false;
    }
    leading_spaces_or_tabs_len(chars.as_str()) > 0
}

fn direct_child_list_indexes(
    lines: &[LineSpan<'_>],
    parent_index: usize,
    block_end: usize,
) -> Vec<usize> {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent_index].text);
    let mut indexes = Vec::new();
    for (offset, line) in lines[parent_index + 1..].iter().enumerate() {
        if line.end > block_end {
            break;
        }
        let line_index = parent_index + 1 + offset;
        if leading_spaces_or_tabs_len(line.text) <= parent_indent
            || list_item_body(line.text).is_none()
            || nearest_shallower_list_item_parent(lines, line_index)
                != Some(parent_index)
        {
            continue;
        }
        indexes.push(line_index);
    }
    indexes
}

fn line_start(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

const COMMAND_NAME: &str = "bob capture-task-sections";

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
    let request = match CaptureTaskSectionsRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_error(error, output_format),
    };

    match list_capture_task_sections(&request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
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
        .about("List the ALL-CAPS child sections of a capture task")
        .long_about(
            "List the ALL-CAPS direct-child section bullets of one task in a \
routed Bob note.\n\n\
The command is read-only and reports every qualifying section in document \
order with its original title, whitespace-free slug, line, child count, and \
depth. It uses the same parent-task lookup and error messages as \
`bob capture`: a missing, duplicated, or non-task block ID is an error, as \
is a stale or ambiguous --task-ref. A resolved task with no sections \
returns a successful empty list so picker callers can skip the chooser.\n\n\
Exactly one of -i/--block-id or -t/--task-ref is required. Use --block-id \
when the parent already has an ID, and --task-ref for the stale-safe picker \
ref from `bob capture-complete` or `bob capture-tasks`.",
        )
        .after_help(
            "Examples:\n  bob capture-task-sections --route foo --block-id bar\n  bob capture-task-sections -r foo -i bar -f json\n  bob capture-task-sections -r cash -t 12:abcd1234\n  bob capture-task-sections -b ~/bob -r project-alpha -i goog-exit\n\nEnvironment:\n  BOB_DIR                    Bob vault root when --bob-dir is omitted",
        )
        .disable_help_flag(true)
        .arg(block_id_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
        .arg(task_ref_arg())
}

fn block_id_arg() -> Arg {
    Arg::new("block-id")
        .long("block-id")
        .short('i')
        .value_name("ID")
        .help("Block ID of the parent task")
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

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Route/name of the note that contains the task")
}

fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .short('t')
        .value_name("REF")
        .help("Stale-safe task ref from capture-complete or capture-tasks")
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParentSelector {
    BlockId(String),
    TaskRef(TaskRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureTaskSectionsRequest {
    bob_dir: PathBuf,
    route: String,
    parent: ParentSelector,
}

impl CaptureTaskSectionsRequest {
    fn from_matches(
        matches: &ArgMatches,
    ) -> Result<Self, CaptureTaskSectionsError> {
        let block_id = matches.get_one::<String>("block-id");
        let task_ref = matches.get_one::<String>("task-ref");
        let parent = match (block_id, task_ref) {
            (Some(block_id), None) => {
                if !capture_language::is_block_id(block_id) {
                    return Err(CaptureTaskSectionsError::usage(
                        "--block-id must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
                    ));
                }
                ParentSelector::BlockId(block_id.clone())
            }
            (None, Some(task_ref)) => {
                let parsed = TaskRef::parse(task_ref).ok_or_else(|| {
                    CaptureTaskSectionsError::usage(
                        "--task-ref must use <line>:<digest>",
                    )
                })?;
                ParentSelector::TaskRef(parsed)
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(CaptureTaskSectionsError::usage(
                    "exactly one of --block-id or --task-ref is required",
                ));
            }
        };

        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            route: route_from_matches(matches)?,
            parent,
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

fn route_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureTaskSectionsError> {
    let Some(route) = matches.get_one::<String>("route") else {
        return Err(CaptureTaskSectionsError::usage("--route is required"));
    };
    if capture::is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }
    Err(CaptureTaskSectionsError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureTaskSectionsResult {
    ok: bool,
    schema_version: u32,
    route: String,
    block_id: Option<String>,
    #[serde(rename = "ref")]
    task_ref: String,
    count: usize,
    sections: Vec<ListedTaskSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListedTaskSection {
    title: String,
    slug: String,
    line: usize,
    child_count: usize,
    depth: usize,
}

fn list_capture_task_sections(
    request: &CaptureTaskSectionsRequest,
) -> Result<CaptureTaskSectionsResult, CaptureTaskSectionsError> {
    let relative_target = capture::route_label(&request.route);
    let target = request.bob_dir.join(&relative_target);
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureTaskSectionsError::io(format!(
                "note does not exist: {}",
                target.display()
            )));
        }
        Err(error) => {
            return Err(CaptureTaskSectionsError::io(format!(
                "read target {}: {error}",
                target.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(&request.bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = resolve_parent(&scan, &request.route, &request.parent)?;
    let sections = task_sections(&contents, parent)
        .into_iter()
        .map(|section| ListedTaskSection {
            title: section.title,
            slug: section.slug,
            line: section.line,
            child_count: section.child_count,
            depth: 1,
        })
        .collect::<Vec<_>>();

    Ok(CaptureTaskSectionsResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        route: request.route.clone(),
        block_id: parent.block_id.clone(),
        task_ref: parent.task_ref(),
        count: sections.len(),
        sections,
    })
}

fn resolve_parent<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    route: &str,
    parent: &ParentSelector,
) -> Result<&'a NoteTask, CaptureTaskSectionsError> {
    match parent {
        ParentSelector::BlockId(block_id) => match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => Ok(task),
            BlockIdLookup::NotATask {
                line_index,
                excerpt,
            } => Err(CaptureTaskSectionsError::io(format!(
                "^{block_id} in {route}.md is not a task (line {}: {excerpt})",
                line_index + 1
            ))),
            BlockIdLookup::Duplicate(count) => {
                Err(CaptureTaskSectionsError::io(format!(
                    "block ID ^{block_id} appears {count} times in {route}.md; make it unique before capturing"
                )))
            }
            BlockIdLookup::Missing => {
                let choices = format!(
                    "run 'bob capture-tasks -r {route}' to list task block IDs"
                );
                let message = match scan.suggest_block_id(block_id) {
                    Some(suggestion) => format!(
                        "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                    ),
                    None => format!(
                        "no task with block ID ^{block_id} in {route}.md ({choices})"
                    ),
                };
                Err(CaptureTaskSectionsError::io(message))
            }
        },
        ParentSelector::TaskRef(task_ref) => {
            match scan.by_ref(task_ref.line, &task_ref.digest) {
                RefLookup::Found(task) => Ok(task),
                RefLookup::Stale => Err(CaptureTaskSectionsError::io(format!(
                    "the selected task is no longer in {route}.md; rerun the task picker"
                ))),
                RefLookup::Ambiguous => {
                    Err(CaptureTaskSectionsError::io(format!(
                        "the selected task matches more than one line in {route}.md; rerun the task picker"
                    )))
                }
            }
        }
    }
}

fn print_success(
    result: &CaptureTaskSectionsResult,
    output_format: OutputFormat,
) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureTaskSectionsResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(
    result: &CaptureTaskSectionsResult,
    styler: &Styler,
) -> String {
    let route_label = capture::route_label(&result.route);
    let mut output = format!(
        "Capture task sections {} {}",
        styler.separator(),
        styler.cyan(&route_label)
    );
    if let Some(block_id) = &result.block_id {
        output.push(' ');
        output.push_str(&styler.cyan(&format!("^{block_id}")));
    }
    output.push_str("\n\n");

    if result.sections.is_empty() {
        output.push_str("  No task sections found.\n");
    } else {
        for section in &result.sections {
            let items = format!(
                "{} {}",
                section.child_count,
                if section.child_count == 1 {
                    "item"
                } else {
                    "items"
                }
            );
            output.push_str(&format!(
                "  {}  {}  {}\n",
                styler.cyan(&section.title),
                styler.dim(&section.slug),
                styler.dim(&items)
            ));
        }
    }

    output.push('\n');
    output.push_str(&format!(
        "{} {}\n",
        result.count,
        if result.count == 1 {
            "section"
        } else {
            "sections"
        }
    ));
    output
}

fn success_json(result: &CaptureTaskSectionsResult) -> String {
    serde_json::to_string(result)
        .expect("serialize capture task sections result")
}

fn print_error(
    error: CaptureTaskSectionsError,
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
struct CaptureTaskSectionsError {
    kind: CaptureTaskSectionsErrorKind,
    message: String,
}

impl CaptureTaskSectionsError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTaskSectionsErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureTaskSectionsErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureTaskSectionsErrorKind {
    Usage,
    Io,
}

impl CaptureTaskSectionsErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::native::note_tasks;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn parent_task(contents: &str) -> NoteTask {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        scan.task_named("Parent")
            .cloned()
            .unwrap_or_else(|| panic!("missing Parent task in {contents:?}"))
    }

    fn sections_of(contents: &str) -> Vec<TaskSection> {
        task_sections(contents, &parent_task(contents))
    }

    fn titles_of(contents: &str) -> Vec<String> {
        sections_of(contents)
            .into_iter()
            .map(|section| section.title)
            .collect()
    }

    fn section_named<'a>(
        sections: &'a [TaskSection],
        title: &str,
    ) -> &'a TaskSection {
        sections
            .iter()
            .find(|section| section.title == title)
            .unwrap_or_else(|| panic!("missing section {title:?}"))
    }

    #[test]
    fn slug_trims_collapses_whitespace_and_lowercases() {
        assert_eq!(slug("REQUIREMENTS"), "requirements");
        assert_eq!(slug("FUTURE WORK"), "future-work");
        assert_eq!(slug("FUTURE  WORK"), "future-work");
        assert_eq!(slug("  Q&A  "), "q&a");
        assert_eq!(slug("NON-GOALS"), "non-goals");
        assert_eq!(slug("WHAT'S NEXT"), "what's-next");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn title_whitelist_edges() {
        assert_eq!(
            parse_task_section_title("1 REQUIREMENTS"),
            Some("1 REQUIREMENTS")
        );
        assert_eq!(
            parse_task_section_title("Q&A (DRAFT), V2.0/OK-GO"),
            Some("Q&A (DRAFT), V2.0/OK-GO")
        );
        assert_eq!(
            parse_task_section_title("WHAT'S NEXT"),
            Some("WHAT'S NEXT")
        );
        assert_eq!(
            parse_task_section_title("FUTURE  WORK"),
            Some("FUTURE  WORK")
        );
        assert_eq!(
            parse_task_section_title("  REQUIREMENTS  "),
            Some("REQUIREMENTS")
        );

        for body in [
            "SNAKE_CASE",
            "requirements",
            "Requirements",
            "123",
            "",
            "[[SOME_NOTE]]",
            "#TODO",
            "REQUIREMENTS ^abc",
            "`CODE`",
            "FOO_BAR",
            "A_B",
        ] {
            assert_eq!(parse_task_section_title(body), None, "{body:?}");
        }

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- 1 REQUIREMENTS\n",
            "\t- Q&A (DRAFT), V2.0/OK-GO\n",
            "\t- FUTURE  WORK\n",
            "\t- SNAKE_CASE\n",
            "\t- requirements\n",
            "\t- 123\n",
            "\t- [[SOME_NOTE]]\n",
            "\t- #TODO\n",
            "\t- REQUIREMENTS ^abc\n",
            "\t- `CODE`\n",
            "\t- NOTES\n",
        );
        assert_eq!(
            titles_of(contents),
            [
                "1 REQUIREMENTS",
                "Q&A (DRAFT), V2.0/OK-GO",
                "FUTURE  WORK",
                "NOTES"
            ]
        );
        assert_eq!(
            sections_of(contents)
                .into_iter()
                .map(|section| section.slug)
                .collect::<Vec<_>>(),
            [
                "1-requirements",
                "q&a-(draft),-v2.0/ok-go",
                "future-work",
                "notes"
            ]
        );
    }

    #[test]
    fn checkboxed_all_caps_children_are_not_sections() {
        assert_eq!(parse_task_section_title("[ ] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[x] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[/] REQUIREMENTS"), None);
        assert_eq!(parse_task_section_title("[x]REQUIREMENTS"), None);

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- [ ] REQUIREMENTS\n",
            "\t- [x] REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        assert_eq!(titles_of(contents), ["FUTURE WORK"]);
    }

    #[test]
    fn grandchild_is_not_a_direct_child_section() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- GRANDCHILD\n",
            "\t- FUTURE WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(section_named(&sections, "REQUIREMENTS").child_count, 1);
        assert_eq!(section_named(&sections, "FUTURE WORK").child_count, 0);
    }

    #[test]
    fn empty_section_bullet_still_qualifies() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "REQUIREMENTS");
        assert_eq!(sections[0].child_count, 0);
        assert_eq!(sections[1].child_count, 0);
    }

    #[test]
    fn ordered_and_star_plus_markers_qualify() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t1. REQUIREMENTS\n",
            "\t* FUTURE WORK\n",
            "\t+ NOTES\n",
            "\t2) Q&A\n",
        );
        assert_eq!(
            titles_of(contents),
            ["REQUIREMENTS", "FUTURE WORK", "NOTES", "Q&A"]
        );
    }

    #[test]
    fn tab_two_space_four_space_and_mixed_indentation() {
        let tab = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
        );
        let tab_sections = sections_of(tab);
        assert_eq!(
            tab_sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(tab_sections[0].indentation, "\t");
        assert_eq!(
            section_insertion(tab, &tab_sections[0]).indentation,
            "\t\t"
        );

        let two_space = concat!(
            "- [ ] #task Parent\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "  - FUTURE WORK\n",
        );
        let two_space_sections = sections_of(two_space);
        assert_eq!(two_space_sections[0].indentation, "  ");
        assert_eq!(
            section_insertion(two_space, &two_space_sections[0]).indentation,
            "    "
        );

        let four_space = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "        - existing\n",
            "    - FUTURE WORK\n",
        );
        let four_space_sections = sections_of(four_space);
        assert_eq!(four_space_sections[0].indentation, "    ");
        assert_eq!(
            section_insertion(four_space, &four_space_sections[0]).indentation,
            "        "
        );

        // Mixed-indent siblings are both direct children via nearest-shallower
        // ancestry; do not collapse to the plugin's shallowest-only rule.
        let mixed = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "  - FUTURE WORK\n",
        );
        assert_eq!(titles_of(mixed), ["REQUIREMENTS", "FUTURE WORK"]);
    }

    #[test]
    fn managed_logs_are_never_sections_plain_titles_are() {
        let recognized = [
            "\t- 🗓️ **SCHEDULE LOG**",
            "\t* **SCHEDULE LOG**",
            "\t+ **SCHEDULE LOG:**",
            "\t1. **Schedule log:**",
            "\t2) 🗓️ **Schedule log**",
            "\t- 🛠️ **WORK LOG**",
            "\t* **WORK LOG**",
            "\t+ **Work log:**",
            "\t10. **Work log**",
            "\t3) 🛠️ **WORK LOG:**",
        ];
        for marker in recognized {
            let contents = format!("- [ ] #task Parent\n{marker}\n\t- NOTES\n");
            assert_eq!(titles_of(&contents), ["NOTES"], "{marker}");
        }

        let plain = concat!(
            "- [ ] #task Parent\n",
            "\t- SCHEDULE LOG\n",
            "\t- WORK LOG\n",
        );
        assert_eq!(titles_of(plain), ["SCHEDULE LOG", "WORK LOG"]);
    }

    #[test]
    fn whole_slug_beats_earlier_prefix_and_first_duplicate_wins() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- FUTURE WORKFLOW\n",
            "\t- FUTURE WORK\n",
            "\t- FUTURE  WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            select_section(&sections, "future-work")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORK")
        );
        assert_eq!(
            select_section(&sections, "FUTURE WORK")
                .map(|section| section.line),
            Some(3)
        );
        assert_eq!(
            select_section(&sections, "future")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORKFLOW")
        );
        assert_eq!(
            select_section(&sections, "future-workflow")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORKFLOW")
        );
        assert_eq!(select_section(&sections, ""), None);
        assert_eq!(select_section(&sections, "absent"), None);
    }

    #[test]
    fn exact_title_match_is_case_insensitive_and_not_a_slug() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- FUTURE WORKFLOW\n",
            "\t- FUTURE WORK\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            select_section_exact(&sections, "Future Work")
                .map(|section| section.title.as_str()),
            Some("FUTURE WORK")
        );
        assert_eq!(select_section_exact(&sections, "future-work"), None);
        assert_eq!(
            match_section(&sections, "Future Work", true)
                .map(|section| section.title.as_str()),
            Some("FUTURE WORK")
        );
        assert_eq!(
            match_section(&sections, "future", false)
                .map(|section| section.title.as_str()),
            Some("FUTURE WORKFLOW")
        );
    }

    #[test]
    fn suggests_unique_nearby_titles_and_slugs() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
            "\t- NOTES\n",
            "\t- NOTE\n",
        );
        let sections = sections_of(contents);
        assert_eq!(
            suggest_section(&sections, "requirments", false)
                .map(|section| section.title.as_str()),
            Some("REQUIREMENTS")
        );
        assert_eq!(
            suggest_section(&sections, "future-work", true)
                .map(|section| section.title.as_str()),
            Some("FUTURE WORK")
        );
        assert_eq!(suggest_section(&sections, "notx", false), None);
        assert_eq!(suggest_section(&sections, "zzz", false), None);
        assert_eq!(suggest_section(&sections, "", false), None);
    }

    #[test]
    fn insertion_geometry_for_middle_last_blank_and_managed_log() {
        let middle = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
            "\t\t- later\n",
            "\t- NOTES\n",
        );
        let sections = sections_of(middle);
        let requirements = section_named(&sections, "REQUIREMENTS");
        let future = section_named(&sections, "FUTURE WORK");
        let notes = section_named(&sections, "NOTES");
        assert_eq!(
            section_insertion(middle, requirements).offset,
            middle.find("\t- FUTURE WORK").expect("middle sibling")
        );
        assert_eq!(
            section_insertion(middle, future).offset,
            middle.find("\t- NOTES").expect("last-but-one sibling")
        );
        assert_eq!(section_insertion(middle, notes).offset, middle.len());
        assert_eq!(notes.child_count, 0);

        let blank = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\n",
            "\t- FUTURE WORK\n",
        );
        let blank_sections = sections_of(blank);
        assert_eq!(
            section_insertion(blank, &blank_sections[0]).offset,
            blank.find("\t- FUTURE WORK").expect("after blank")
        );

        let managed = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-08-01* — scheduled\n",
            "\t- FUTURE WORK\n",
        );
        let managed_sections = sections_of(managed);
        let managed_requirements =
            section_named(&managed_sections, "REQUIREMENTS");
        assert_eq!(managed_requirements.child_count, 1);
        assert_eq!(
            section_insertion(managed, managed_requirements).offset,
            managed
                .find("\t\t- 🗓️ **SCHEDULE LOG**")
                .expect("managed log")
        );
        assert_eq!(
            section_insertion(managed, managed_requirements).indentation,
            "\t\t"
        );
    }

    #[test]
    fn insertion_geometry_preserves_crlf_offsets() {
        let contents = concat!(
            "- [ ] #task Parent\r\n",
            "\t- REQUIREMENTS\r\n",
            "\t\t- existing\r\n",
            "\t- FUTURE WORK\r\n",
        );
        let sections = sections_of(contents);
        let requirements = section_named(&sections, "REQUIREMENTS");
        let offset = section_insertion(contents, requirements).offset;
        assert_eq!(
            offset,
            contents.find("\t- FUTURE WORK").expect("crlf sibling")
        );
        assert!(contents[..offset].ends_with("\r\n"));
        assert_eq!(
            section_insertion(
                contents,
                section_named(&sections, "FUTURE WORK")
            )
            .offset,
            contents.len()
        );
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn lists_sections_in_document_order_for_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-task-sections-list");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            concat!(
                "# Tasks\n",
                "- [ ] #task Parent ^bar\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing\n",
                "\t- FUTURE WORK\n",
                "\t- NOTES\n",
            ),
        );

        let result = list_capture_task_sections(&request(
            temp.path(),
            "Foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect("list");

        assert_eq!(result.schema_version, 1);
        assert_eq!(result.route, "foo");
        assert_eq!(result.block_id.as_deref(), Some("bar"));
        assert_eq!(result.count, 3);
        assert_eq!(result.sections[0].title, "REQUIREMENTS");
        assert_eq!(result.sections[0].slug, "requirements");
        assert_eq!(result.sections[0].line, 3);
        assert_eq!(result.sections[0].child_count, 1);
        assert_eq!(result.sections[0].depth, 1);
        assert_eq!(result.sections[1].title, "FUTURE WORK");
        assert_eq!(result.sections[1].slug, "future-work");
        assert_eq!(result.sections[2].title, "NOTES");
        assert_eq!(result.sections[2].child_count, 0);
        assert!(result.task_ref.starts_with("2:"));
    }

    #[test]
    fn resolved_task_with_no_sections_is_a_successful_empty_list() {
        let temp = TempDir::new("bob-cli-capture-task-sections-empty");
        write_settings(temp.path());
        write_file(&temp.path().join("foo.md"), "- [ ] #task Parent ^bar\n");

        let result = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect("empty list");

        assert_eq!(result.count, 0);
        assert!(result.sections.is_empty());
        let human = human_success(&result, &Styler::plain());
        assert!(human.contains("No task sections found."), "{human}");
        assert!(human.contains("0 sections"), "{human}");
    }

    #[test]
    fn missing_note_and_unresolvable_parents_are_errors() {
        let temp = TempDir::new("bob-cli-capture-task-sections-errors");
        write_settings(temp.path());
        let original = concat!(
            "Plain heading ^plain-id\n",
            "- [ ] #task Ready ^ready-id\n",
            "- [ ] #task Dup ^dup-id\n",
            "- [ ] #task Also dup ^dup-id\n",
            "- [ ] #task Same\n",
            "- [ ] #task Same\n",
        );
        write_file(&temp.path().join("foo.md"), original);
        let same_digest = task_ref_for(original, "Same").digest;

        let missing_note = list_capture_task_sections(&request(
            temp.path(),
            "missing",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect_err("missing note");
        assert_eq!(missing_note.kind, CaptureTaskSectionsErrorKind::Io);
        assert!(
            missing_note.message.contains("does not exist"),
            "{}",
            missing_note.message
        );

        let missing = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect_err("missing id");
        assert!(
            missing.message.contains("no task with block ID ^bar")
                && missing.message.contains("capture-tasks -r foo"),
            "{}",
            missing.message
        );

        let close = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("ready-i".to_string()),
        ))
        .expect_err("close match");
        assert!(
            close.message.contains("did you mean ^ready-id"),
            "{}",
            close.message
        );

        let duplicate = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("dup-id".to_string()),
        ))
        .expect_err("duplicate");
        assert!(
            duplicate.message.contains("appears 2 times"),
            "{}",
            duplicate.message
        );

        let not_a_task = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("plain-id".to_string()),
        ))
        .expect_err("not a task");
        assert!(
            not_a_task.message.contains("is not a task")
                && not_a_task.message.contains("Plain heading"),
            "{}",
            not_a_task.message
        );

        let stale = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(TaskRef {
                line: 99,
                digest: "deadbeef".to_string(),
            }),
        ))
        .expect_err("stale");
        assert!(
            stale.message.contains("no longer in foo.md"),
            "{}",
            stale.message
        );

        let ambiguous = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(TaskRef {
                line: 99,
                digest: same_digest,
            }),
        ))
        .expect_err("ambiguous");
        assert!(
            ambiguous.message.contains("matches more than one line"),
            "{}",
            ambiguous.message
        );
    }

    #[test]
    fn task_ref_resolves_a_task_without_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-task-sections-ref");
        write_settings(temp.path());
        let original =
            concat!("# Tasks\n", "- [ ] #task Parent\n", "\t- REQUIREMENTS\n",);
        write_file(&temp.path().join("foo.md"), original);
        let task_ref = task_ref_for(original, "Parent");

        let result = list_capture_task_sections(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(task_ref),
        ))
        .expect("list by ref");

        assert!(result.block_id.is_none());
        assert_eq!(result.count, 1);
        assert_eq!(result.sections[0].title, "REQUIREMENTS");
        assert_eq!(
            result.task_ref,
            task_ref_for(original, "Parent").to_string()
        );
    }

    #[test]
    fn request_validation_covers_route_and_exclusive_selectors() {
        let both = parse_request(&[
            "--route",
            "foo",
            "--block-id",
            "bar",
            "--task-ref",
            "1:abcd1234",
        ])
        .expect_err("both");
        assert_eq!(both.kind, CaptureTaskSectionsErrorKind::Usage);
        assert!(
            both.message
                .contains("exactly one of --block-id or --task-ref"),
            "{}",
            both.message
        );

        let neither = parse_request(&["--route", "foo"]).expect_err("neither");
        assert!(
            neither
                .message
                .contains("exactly one of --block-id or --task-ref"),
            "{}",
            neither.message
        );

        let missing_route =
            parse_request(&["--block-id", "bar"]).expect_err("route");
        assert_eq!(missing_route.message, "--route is required");

        let invalid_route =
            parse_request(&["--route", "../bad", "--block-id", "bar"])
                .expect_err("invalid route");
        assert!(invalid_route.message.contains("must contain only"));

        let invalid_id =
            parse_request(&["--route", "foo", "--block-id", "bad.id"])
                .expect_err("invalid id");
        assert!(invalid_id.message.contains("--block-id must"));

        let invalid_ref =
            parse_request(&["--route", "foo", "--task-ref", "nope"])
                .expect_err("invalid ref");
        assert!(invalid_ref.message.contains("--task-ref must use"));
    }

    #[test]
    fn json_success_shape_and_key_order_are_stable() {
        let result = CaptureTaskSectionsResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            route: "foo".to_string(),
            block_id: Some("bar".to_string()),
            task_ref: "2:abcd1234".to_string(),
            count: 1,
            sections: vec![ListedTaskSection {
                title: "REQUIREMENTS".to_string(),
                slug: "requirements".to_string(),
                line: 3,
                child_count: 1,
                depth: 1,
            }],
        };

        let raw = success_json(&result);
        assert_key_order(
            &raw,
            &[
                "\"ok\"",
                "\"schema_version\"",
                "\"route\"",
                "\"block_id\"",
                "\"ref\"",
                "\"count\"",
                "\"sections\"",
            ],
        );
        let section_start = raw.find("\"sections\"").expect("sections");
        assert_key_order(
            &raw[section_start..],
            &[
                "\"title\"",
                "\"slug\"",
                "\"line\"",
                "\"child_count\"",
                "\"depth\"",
            ],
        );

        let value: serde_json::Value =
            serde_json::from_str(&raw).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["route"], "foo");
        assert_eq!(value["block_id"], "bar");
        assert_eq!(value["ref"], "2:abcd1234");
        assert_eq!(value["count"], 1);
        assert_eq!(value["sections"][0]["title"], "REQUIREMENTS");
        assert_eq!(value["sections"][0]["slug"], "requirements");
        assert_eq!(value["sections"][0]["line"], 3);
        assert_eq!(value["sections"][0]["child_count"], 1);
        assert_eq!(value["sections"][0]["depth"], 1);
    }

    fn request(
        bob_dir: &Path,
        route: &str,
        parent: ParentSelector,
    ) -> CaptureTaskSectionsRequest {
        CaptureTaskSectionsRequest {
            bob_dir: bob_dir.to_path_buf(),
            route: route.to_ascii_lowercase(),
            parent,
        }
    }

    fn parse_request(
        args: &[&str],
    ) -> Result<CaptureTaskSectionsRequest, CaptureTaskSectionsError> {
        let matches = build_cli()
            .try_get_matches_from(
                std::iter::once(COMMAND_NAME.to_string())
                    .chain(args.iter().map(|arg| (*arg).to_string())),
            )
            .expect("clap parse");
        CaptureTaskSectionsRequest::from_matches(&matches)
    }

    fn task_ref_for(contents: &str, description: &str) -> TaskRef {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        let task = scan
            .task_named(description)
            .unwrap_or_else(|| panic!("missing task {description}"));
        TaskRef::from_task(task)
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
                "customStatuses": []
              }
            }"##,
        );
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    fn assert_key_order(text: &str, needles: &[&str]) {
        let mut last = 0;
        for needle in needles {
            let position = text[last..].find(needle).unwrap_or_else(|| {
                panic!("expected `{needle}` after {last} in {text}")
            }) + last;
            last = position + needle.len();
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
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
}
