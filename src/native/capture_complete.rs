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
    capture, capture_active_tasks, capture_block_ids,
    capture_language::{self, CompletionContext},
    capture_link_tasks,
    capture_links::{
        self, WikilinkBlockCandidate, WikilinkHeadingCandidate,
        WikilinkNoteCandidate,
    },
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    capture_targets::{self, CaptureTargetKind},
    capture_task_sections, capture_tasks, config, env as bob_env,
    note_tasks::{self, BlockIdLookup},
    plan_budget, pomodoro,
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

pub(crate) fn build_cli() -> ClapCommand {
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
'@route:id#', '@route:#prefix', '@route^id+#prefix', and a bare '@route^id+#'; \
it is backed by `bob \
capture-pomodoros`, offers only open entries, collapses duplicate named \
slugs, and keeps nameable rows after named rows even when the query is \
nonempty. Nameable rows set requires_name and use an empty replacement that \
updated clients must not insert. When the query is a nonempty valid \
Pomodoro name that would not select an open exact or prefix match, and \
today's ledger can uniquely place a new future entry, the first candidate \
is a create action: creates_pomodoro is true, replacement is the canonical \
selector, name is the canonical visible name, and ref is omitted. Create \
rows also preview the plan budget with plan_themes_after and \
plan_themes_cap (omitted when the daily note or the plan config is \
unavailable). Accepting \
that row only canonicalizes the marker; `bob capture` creates the named \
placeholder later. Exact or prefix open-name matches stay first and do not \
receive a create row. Empty queries stay the existing discovery list. \
Pomodoro-start-name completion covers the name part of a `=<X>#name` named \
start and the `=<X>#` incomplete state, backed by today's ledger: start \
rows for planned placeholders (the entry a bare `=` would start carries \
`next_up`), a create row for a missing name, `again` rows that start a \
new session named like a completed one, `name it` rows for placeholders \
that still need a name, and the running entry last. Its new and again rows \
preview the plan budget with the same `plan_themes_after`/`plan_themes_cap` \
fields. The `pomodoro_name` context is unchanged. A \
missing daily note, a missing Pomodoros section, and multiple open timed \
Pomodoros stay write-free warnings without a create row. On a `@<route>:<block-id>[#<name>]=<X>` marker the `=<X>` start suffix is \
never completable: block and Pomodoro-name replacement ranges end before \
the `=`, a cursor inside the suffix returns an empty success, and accepting \
a candidate preserves the typed suffix. The `=x[<N>][*<P>][!<M>][~<K>]` close suffix (including the `=*`/`=!` aliases) behaves the \
same way: it is never a completion field, replacements still stop before \
`#`/`=`, and a cursor anywhere inside the suffix, including the task-number \
lists and a dangling `,`/`~` separator, returns an empty success. A whole-item \
`+[N]`/`-[N]` Pomodoro adjustment, `++[N]`/`--[N]` Pomodoro shift (a bare `+`, `-`, `++`, or `--` is one unit), `=x[<N>][*<P>][!<M>][~<K>]`/`=*[<P>]`/`=![<M>]` close, or a bare `=`/`=<X>` start (a bare `=` starts 25 minutes) is an action and requests no route or task completion candidates: a cursor on such an item returns an empty success. Work Log text completes like a bullet line whether it sits below the close or on it: inline entry text requests no marker, `:`/`^`, or `wikilink_block` candidates, while note and heading wikilinks keep completing. A `=<X>#name` named start instead completes the name after `#` as `pomodoro_start_name`, per token inside chains; a cursor on `=<X>` or at the `#` byte itself returns an empty success, the `pomodoro_start_name` replacement range ends before a trailing `~<K>` drop part so accepting a name keeps the typed list, and a cursor anywhere inside the drop part, including a dangling `~`/`,` separator, returns an empty success. Pomodoro block-ID \
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
each group. A solo leading '^' token completes active tasks instead: the \
`route:block-id` part offers In Progress and Next tasks with block IDs, \
ordered by today's open-Pomodoro Task Links (queued first, then In \
Progress, then Next), and accepting a row inserts the full \
`route:block-id` in one step while a typed `#name`/`=<X>` suffix \
survives. A `#name` after `^route:block-id` completes Pomodoro names \
exactly as it does after `@route:block-id`, and a cursor inside `=<X>` \
or `=x[<N>][*<P>][!<M>][~<K>]`/`=*`/`=!` offers nothing. A solo leading `:` token \
completes `task_link`: every linkable open task (Ready, Blocked, Next, In \
Progress) in the routable inbox, area, and non-terminal project notes, in \
canonical picker order and ranked by the query with ID-less tasks always \
included; the replacement covers the whole `:` token, sigil included, \
because accepting rewrites the query into the canonical `@route:block-id` \
link. The right-hand side of '@route^block-id' completes as `task_block_id` once the route resolves, with empty \
candidates and an additive `block_id` object carrying intent, used IDs, and suggestions. A project-note `+` \
directly after the `^` block-ID part is never part of the replacement; a `+` after a `:` block ID is the retired project-note form and offers nothing. A trailing ` :id` or ` ^id` on a \
first-level bullet of a project-note item completes as `project_task_block_id`, with empty candidates and an additive `block_id` object carrying intent `new`, the project-note stem, the sigil range, the bullet body, used IDs (`prj` plus sibling task IDs), and suggestions. An empty block-ID component \
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
            "Examples:\n  bob capture-complete --cursor 1 -- '@'\n  bob capture-complete -c 4 -- '@@fo'\n  bob capture-complete -c 20 -- 'Buy milk @@gro'\n  bob capture-complete -c 19 -f json -- 'jot idea @notes#Id'\n  bob capture-complete -c 20 -f json -- 'Fix flaky test @sase^'\n  bob capture-complete -c 12 -b ~/bob -- 'Do work @Dev^new-id'\n  bob capture-complete -c 16 -b ~/bob -- 'Do work @Dev:foc'\n  bob capture-complete -c 16 -b ~/bob -- 'note @foo+bar#'\n  bob capture-complete -a -c 6 -f json -- '@file+'\n  bob capture-complete -a -c 8 -f json -- '@@file+'\n  bob capture-complete -c 5 -- '[[sas'\n  bob capture-complete -c 1 -- '^'\n  bob capture-complete -c 1 -- ':'\n\nContexts:\n  route, section, pomodoro_block_id, task_block_id, project_task_block_id, pomodoro_name, pomodoro_start_name, task, task_section, active_task, task_link, wikilink_note, wikilink_heading, wikilink_block",
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
    line: usize,
    pomodoro: Option<ActiveTaskPomodoroCandidate>,
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
struct ActiveTaskPomodoroCandidate {
    line: usize,
    name: Option<String>,
    time_range: Option<String>,
    is_current: bool,
}

/// One `task_link` (`:` picker) candidate: any linkable open task in a
/// routable inbox, area, or non-terminal project note. The JSON keys match
/// the picker contract: `replacement` is the `@route:id` an accept inserts
/// (empty for ID-less tasks, which clients must never insert),
/// `requires_block_id` marks those rows, and `pulls_forward` is omitted
/// when false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TaskLinkCandidate {
    replacement: String,
    #[serde(rename = "ref")]
    task_ref: String,
    route: String,
    note_kind: CaptureTargetKind,
    block_id: Option<String>,
    requires_block_id: bool,
    block_id_suggestions: Vec<String>,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    depth: usize,
    line: usize,
    group: capture_link_tasks::LinkTaskGroup,
    scheduled: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pulls_forward: bool,
    pomodoro: Option<ActiveTaskPomodoroCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ActiveTaskCandidate {
    replacement: String,
    #[serde(rename = "ref")]
    task_ref: String,
    route: String,
    block_id: String,
    status_symbol: char,
    status_name: String,
    status_type: &'static str,
    text: String,
    section: Option<String>,
    pomodoro: Option<ActiveTaskPomodoroCandidate>,
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
    /// Marks the row `bob capture` with a bare `=` would start. Only set
    /// in the `pomodoro_start_name` context; always false (and therefore
    /// omitted) in the `pomodoro_name` context so that output stays
    /// byte-identical.
    #[serde(skip_serializing_if = "is_false")]
    next_up: bool,
    /// Resulting theme count and cap when this create row is accepted.
    /// Only on `creates_pomodoro` rows (new and again rows in
    /// `pomodoro_start_name`); omitted when the daily note or the plan
    /// config is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_themes_after: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_themes_cap: Option<u32>,
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
    ActiveTask(Vec<ActiveTaskCandidate>),
    TaskLink(Vec<TaskLinkCandidate>),
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
            Self::ActiveTask(items) => items.len(),
            Self::TaskLink(items) => items.len(),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<capture_block_ids::BlockIdField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// `true` when `cursor` sits in Work Log text: a bullet line or an inline
/// entry on the close line. Block-link completion is suppressed there
/// because entries reject block links, while note and heading completion
/// keeps working.
fn cursor_in_close_log_text(raw_text: &str, cursor: usize) -> bool {
    capture_language::cursor_in_close_log_text(raw_text, cursor)
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
            block_id: None,
            warnings: Vec::new(),
        }
    }
}

/// One shell-completion row: the full marker text to insert (from the
/// marker's `@`/`^`/`:`/`=` sigil through the candidate), a description,
/// a human group, and whether the shell should keep typing (`nospace`)
/// because the value ends in a continuation character (`:` `+` `#` `=`
/// `^`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellRow {
    pub full: String,
    pub description: String,
    pub group: String,
    pub nospace: bool,
}

/// Shell completion for one capture marker: the marker's byte start in
/// `raw_text` plus safe rows. `None` means no marker (or a deferred
/// wikilink): the caller falls back or shows nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCompletion {
    pub marker_start: usize,
    pub rows: Vec<ShellRow>,
}

/// Extract shell completion from `capture_complete` without re-implementing
/// grammar: wikilinks return `None` before `NoteIndex::read` (about 300 ms,
/// deferred), safe rows only (no `requires_block_id` / `requires_name`;
/// `creates_pomodoro` rows stay as `new Pomodoro`), and nothing beyond
/// completion (no `capture-task-id`, `capture-pomodoro-name`, writes, or
/// dry runs — only `build_result`'s read-only scans with `all_tasks`
/// off).
pub(crate) fn shell_completion(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
) -> Result<Option<ShellCompletion>, String> {
    if raw_text.contains(['\n', '\r']) {
        return Ok(None);
    }
    if cursor > raw_text.len() || !raw_text.is_char_boundary(cursor) {
        return Ok(None);
    }
    // Wikilinks are deferred: return before the ~300 ms `NoteIndex::read`.
    let current_route = capture_language::editor_item_at(raw_text, cursor)
        .and_then(|item| item.route);
    let current_note_path = current_route
        .as_deref()
        .map(capture::route_label)
        .unwrap_or_else(|| capture::route_label(capture::inbox_route()));
    if capture_links::completion_field_at(raw_text, cursor, current_note_path)
        .is_some()
    {
        return Ok(None);
    }
    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(None);
    };
    // End of the active word only: the replacement must end at the cursor.
    // A non-empty `--suffix` never reaches here with end == cursor, since
    // the caller builds `raw_text` ending at the cursor.
    if field.replacement.1 != cursor {
        return Ok(None);
    }
    if field.replacement.0 > field.replacement.1 {
        return Ok(None);
    }
    let Some(prefix) = raw_text.get(..field.replacement.0) else {
        return Ok(None);
    };
    let marker_start = prefix
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    if !raw_text.is_char_boundary(marker_start) {
        return Ok(None);
    }
    let Some(marker_prefix) = raw_text.get(marker_start..field.replacement.0)
    else {
        return Ok(None);
    };
    let marker_prefix = marker_prefix.to_string();
    // The shell filters, not bob: always serve the slot's full set with an
    // empty query, so `matcher-list` keeps working (`@ca` lists every route).
    let route_for_group = field.route.clone().unwrap_or_default();
    let mut rows = Vec::new();
    match field.context {
        CompletionContext::Route => {
            let candidates = route_candidates(bob_dir, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Route(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                let (group, description) = match item.kind {
                    CaptureTargetKind::Inbox => (
                        "inbox",
                        "inbox \u{00B7} default capture target".to_string(),
                    ),
                    CaptureTargetKind::Area => ("areas", "area".to_string()),
                    CaptureTargetKind::Project => (
                        "projects",
                        item.status
                            .as_deref()
                            .map(|status| format!("project \u{00B7} {status}"))
                            .unwrap_or_else(|| "project".to_string()),
                    ),
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: group.to_string(),
                });
            }
        }
        CompletionContext::Section => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let candidates = section_candidates(bob_dir, route, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Section(items) = candidates else {
                return Ok(None);
            };
            let group = format!("sections in {route_for_group}");
            for item in items {
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: format!("H{}", item.level),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::Task => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let candidates = task_candidates(
                bob_dir,
                route,
                "",
                false,
                TaskSearch::MultiField,
            )
            .map_err(|error| error.message.clone())?;
            let Candidates::Task(items) = candidates else {
                return Ok(None);
            };
            let group = format!("tasks in {route_for_group}");
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::TaskSection => {
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let (candidates, _) = task_section_candidates(
                bob_dir,
                route,
                field.block_id.as_deref(),
                "",
            )
            .map_err(|error| error.message.clone())?;
            let Candidates::TaskSection(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "task sections".to_string(),
                });
            }
        }
        CompletionContext::PomodoroName
        | CompletionContext::PomodoroStartName => {
            let (candidates, _) =
                if field.context == CompletionContext::PomodoroName {
                    pomodoro_name_candidates(bob_dir, "")
                        .map_err(|error| error.message.clone())?
                } else {
                    pomodoro_start_name_candidates(bob_dir, "")
                        .map_err(|error| error.message.clone())?
                };
            let Candidates::PomodoroName(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.requires_name || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                let description = if item.creates_pomodoro {
                    "new Pomodoro".to_string()
                } else {
                    pomodoro_description(&item.time_range, &item.name)
                };
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description,
                    group: "open Pomodoros".to_string(),
                });
            }
        }
        CompletionContext::ActiveTask => {
            let (candidates, _) = active_task_candidates(bob_dir, "");
            let Candidates::ActiveTask(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "active tasks".to_string(),
                });
            }
        }
        CompletionContext::TaskLink => {
            let (candidates, _) = task_link_candidates(bob_dir, "");
            let Candidates::TaskLink(items) = candidates else {
                return Ok(None);
            };
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                // The `:` field covers the whole token including the sigil,
                // so `marker_prefix` is empty and the full value is the
                // `@route:id` candidate itself.
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: "active tasks".to_string(),
                });
            }
        }
        CompletionContext::PomodoroBlockId => {
            // Shell completion always offers linkable tasks for `@route:`,
            // even when the whole item would parse as a new `pomodoro_task`:
            // the user completing a block ID wants existing tasks, and all
            // rows are safe (identified, linkable). A `+` after a `:` block
            // ID is the retired project-note form, which offers nothing.
            if raw_text
                .get(field.replacement.1..)
                .is_some_and(|rest| rest.starts_with('+'))
            {
                return Ok(Some(ShellCompletion { marker_start, rows }));
            }
            let Some(route) = field.route.as_deref() else {
                return Ok(None);
            };
            let (candidates, _) = link_candidates(bob_dir, route, "")
                .map_err(|error| error.message.clone())?;
            let Candidates::Task(items) = candidates else {
                return Ok(None);
            };
            let group = format!("tasks in {route_for_group}");
            for item in items {
                if item.requires_block_id || item.replacement.is_empty() {
                    continue;
                }
                let full = format!("{}{}", marker_prefix, item.replacement);
                if full.is_empty() {
                    continue;
                }
                rows.push(ShellRow {
                    nospace: ends_in_continuation(&full),
                    full,
                    description: item.text.clone(),
                    group: group.clone(),
                });
            }
        }
        CompletionContext::TaskBlockId
        | CompletionContext::ProjectTaskBlockId => {
            // A new ID (`@route^id`), a project-note `+`, or a trailing
            // ` :id` / ` ^id`: no safe rows (suggestions live in the
            // additive `block_id` object, not candidates).
            return Ok(Some(ShellCompletion { marker_start, rows }));
        }
        CompletionContext::WikilinkNote
        | CompletionContext::WikilinkHeading
        | CompletionContext::WikilinkBlock => return Ok(None),
    }
    Ok(Some(ShellCompletion { marker_start, rows }))
}

/// A value ending in a continuation character expects more typing.
fn ends_in_continuation(value: &str) -> bool {
    value.ends_with([':', '+', '#', '=', '^'])
}

/// `time · name` for Pomodoro rows, mirroring the vault provider.
fn pomodoro_description(
    time_range: &Option<String>,
    name: &Option<String>,
) -> String {
    match (time_range, name) {
        (Some(time), Some(name)) => format!("{time} \u{00B7} {name}"),
        (Some(time), None) => format!("{time} \u{00B7} open"),
        (None, Some(name)) => name.clone(),
        (None, None) => "open".to_string(),
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
        // In Work Log text, block-link candidates are suppressed because
        // entries reject block links; note and heading completion keeps
        // working.
        if matches!(field.context, CompletionContext::WikilinkBlock)
            && cursor_in_close_log_text(raw_text, cursor)
        {
            return Ok(CaptureCompleteResult::empty(cursor));
        }
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
            | CompletionContext::TaskBlockId
            | CompletionContext::ProjectTaskBlockId
            | CompletionContext::PomodoroName
            | CompletionContext::PomodoroStartName
            | CompletionContext::Task
            | CompletionContext::TaskSection
            | CompletionContext::ActiveTask
            | CompletionContext::TaskLink => {
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
            block_id: None,
            warnings: index.warnings(),
        });
    }

    let Some(field) = capture_language::completion_field_at(raw_text, cursor)
    else {
        return Ok(CaptureCompleteResult::empty(cursor));
    };

    if matches!(
        field.context,
        CompletionContext::PomodoroBlockId
            | CompletionContext::TaskBlockId
            | CompletionContext::ProjectTaskBlockId
    ) {
        let route = field.route.as_deref().expect("route resolved");
        let block_field = capture_block_ids::build_block_id_field(
            bob_dir,
            raw_text,
            cursor,
            &capture_block_ids::BlockIdRequest {
                route,
                replacement: field.replacement,
                context: field.context,
            },
        );
        let (candidates, mut warnings) = match block_field.intent {
            capture_block_ids::BlockIdIntent::Link => {
                if matches!(field.context, CompletionContext::PomodoroBlockId) {
                    link_candidates(bob_dir, route, &field.query)?
                } else {
                    (Candidates::Task(Vec::new()), Vec::new())
                }
            }
            capture_block_ids::BlockIdIntent::New
            | capture_block_ids::BlockIdIntent::ProjectNote => {
                (Candidates::Task(Vec::new()), Vec::new())
            }
        };
        // Surface bounded ledger warnings for link candidates.
        if matches!(block_field.intent, capture_block_ids::BlockIdIntent::Link)
            && matches!(field.context, CompletionContext::PomodoroBlockId)
        {
            let day_file = pomodoro::day_file_for(bob_dir);
            let mut ledger_warnings = Vec::new();
            let _ = capture_active_tasks::read_ledger(
                &day_file,
                &mut ledger_warnings,
            );
            for warning in ledger_warnings {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
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
            block_id: Some(block_field),
            warnings,
        });
    }

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
        CompletionContext::PomodoroBlockId
        | CompletionContext::TaskBlockId
        | CompletionContext::ProjectTaskBlockId => {
            unreachable!("block-id handled above")
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
        CompletionContext::PomodoroStartName => {
            pomodoro_start_name_candidates(bob_dir, &field.query)?
        }
        CompletionContext::ActiveTask => {
            active_task_candidates(bob_dir, &field.query)
        }
        CompletionContext::TaskLink => {
            task_link_candidates(bob_dir, &field.query)
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
        block_id: None,
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
                    line: task.line_index + 1,
                    pomodoro: None,
                }
            })
            .collect(),
    ))
}

/// Link-only candidates for `pomodoro_block_id` with `link` intent:
/// identified open tasks the link path accepts (Ready, Blocked, Next, In
/// Progress via the same predicate the link resolver uses), annotated with
/// the queued Pomodoro from today's ledger.
fn link_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let linkable = scan.open_tasks().filter(|task| {
        task.block_id.is_some()
            && capture_link_tasks::is_linkable_status(task.status_symbol)
    });
    let ranked =
        rank_task_group(linkable.collect(), query, TaskSearch::BlockIdOnly);
    let day_file = pomodoro::day_file_for(bob_dir);
    let mut warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(&day_file, &mut warnings);
    let candidates = ranked
        .into_iter()
        .map(|task| {
            let block_id =
                task.block_id.clone().expect("filtered to identified tasks");
            let pomodoro = ledger
                .owners
                .get(&(route.to_string(), block_id.clone()))
                .map(|entry| ActiveTaskPomodoroCandidate {
                    line: entry.line,
                    name: entry.name.clone(),
                    time_range: entry.time_range.clone(),
                    is_current: entry.is_current,
                });
            TaskCandidate {
                replacement: block_id.clone(),
                task_ref: task.task_ref(),
                block_id: Some(block_id),
                route: route.to_string(),
                requires_block_id: false,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.description.clone(),
                section: task.section.clone(),
                depth: capture_tasks::indentation_depth(&task.indentation),
                child_count: task.child_count,
                line: task.line_index + 1,
                pomodoro,
            }
        })
        .collect();
    Ok((Candidates::Task(candidates), warnings))
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

/// Active-task candidates for a solo leading `^` token: In Progress and
/// Next tasks with block IDs, ordered by today's open-Pomodoro Task Links
/// and ranked by the query. The `replacement` is the `route:block-id` an
/// accept inserts; `pomodoro` is the queued entry, or `null` when the task
/// is not queued.
fn active_task_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_active_tasks::discover(bob_dir);
    let candidates = capture_active_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| ActiveTaskCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
        })
        .collect();
    (Candidates::ActiveTask(candidates), discovered.warnings)
}

/// `task_link` candidates for a solo leading `:` token: every linkable
/// open task (Ready, Blocked, Next, In Progress) in the routable inbox,
/// area, and non-terminal project notes, in canonical order and ranked by
/// the query. ID-less tasks are always included; `--all-tasks` does not
/// affect this context. The `replacement` covers the whole `:` token,
/// sigil included, because accepting rewrites the query into the canonical
/// `@route:block-id` marker.
fn task_link_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| TaskLinkCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            note_kind: task.note_kind,
            block_id: task.block_id.clone(),
            requires_block_id: task.block_id.is_none(),
            block_id_suggestions: task.block_id_suggestions.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            group: task.group,
            scheduled: task.scheduled.clone(),
            pulls_forward: task.pulls_forward,
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
        })
        .collect();
    (Candidates::TaskLink(candidates), discovered.warnings)
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
    let daily_key = plan_budget::daily_key_from_path(day_file);
    let plan_hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    let candidates =
        pomodoro_name_candidates_from_scan_with_hint(&scan, query, plan_hint);

    Ok((Candidates::PomodoroName(candidates), warnings))
}

fn pomodoro_start_name_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = pomodoro::day_file_for(bob_dir);
    pomodoro_start_name_candidates_at(&day_file, query)
}

fn pomodoro_start_name_candidates_at(
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
    let daily_key = plan_budget::daily_key_from_path(day_file);
    let plan_hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    let candidates = pomodoro_start_name_candidates_from_scan_with_hint(
        &scan, query, plan_hint,
    );

    Ok((Candidates::PomodoroName(candidates), warnings))
}

#[cfg(test)]
fn pomodoro_start_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    pomodoro_start_name_candidates_from_scan_with_hint(scan, query, None)
}

fn pomodoro_start_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    let next_up_line =
        capture_pomodoros::next_future_pomodoro(scan).map(|entry| entry.line);

    let mut seen_slugs = Vec::<&str>::new();
    let mut start = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_some()
            || !entry.placeholder
            || !entry.selectable
            || seen_slugs.contains(&entry.slug.as_str())
        {
            continue;
        }
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Open
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        seen_slugs.push(&entry.slug);
        start.push(pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            next_up_line == Some(entry.line),
        ));
    }
    let start = rank(start, query, |candidate| candidate.replacement.as_str());

    let open_slugs = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .map(|entry| entry.slug.as_str())
        .collect::<Vec<_>>();
    // Deduplicate completed slugs keeping the latest in document order,
    // then list the most recent first so an empty query resurfaces the
    // last session first.
    let mut again_entries = Vec::<&PomodoroEntry>::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Completed
            || !entry.selectable
            || open_slugs.contains(&entry.slug.as_str())
        {
            continue;
        }
        if let Some(position) = again_entries
            .iter()
            .position(|existing| existing.slug == entry.slug)
        {
            again_entries[position] = entry;
        } else {
            again_entries.push(entry);
        }
    }
    again_entries.reverse();
    let mut again = Vec::new();
    for entry in again_entries {
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Completed
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        let mut candidate = pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            false,
        );
        // An "again" row starts a new session named like the completed
        // one, so it creates and reports that entry's history.
        candidate.replacement = entry.slug.clone();
        candidate.name = capture_pomodoros::canonicalize_pomodoro_name(
            entry.name.as_deref().unwrap_or(&entry.slug),
        )
        .or_else(|| entry.name.clone());
        candidate.creates_pomodoro = true;
        let display =
            candidate.name.clone().unwrap_or_else(|| entry.slug.clone());
        (candidate.plan_themes_after, candidate.plan_themes_cap) =
            plan_themes_after_for(plan_hint.as_ref(), &display);
        again.push(candidate);
    }
    let again = rank(again, query, |candidate| candidate.replacement.as_str());

    let mut combined = start;
    if let Some(creation) =
        pomodoro_start_creation_candidate(scan, query, plan_hint.as_ref())
    {
        insert_pomodoro_creation_candidate(&mut combined, creation, query);
    }
    combined.extend(again);

    combined.extend(scan.entries.iter().filter_map(|entry| {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_some()
            || !entry.placeholder
            || entry.selectable
        {
            return None;
        }
        Some(pomodoro_name_candidate_with_next_up(
            entry,
            true,
            1,
            next_up_line == Some(entry.line),
        ))
    }));

    combined.extend(scan.entries.iter().filter_map(|entry| {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_none()
            || !entry.selectable
        {
            return None;
        }
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Open
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        Some(pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            false,
        ))
    }));

    combined
}

/// The start-specific create row: only a `Missing` query may create, so a
/// completed-only match ("again") never also offers a duplicate create
/// row. Placement, naming, and plan-budget preview reuse the shared link
/// helper; `named_creation_name` stays untouched for link-form callers.
fn pomodoro_start_creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<&PlanCreationHint>,
) -> Option<PomodoroNameCandidate> {
    if query.is_empty() {
        return None;
    }
    if !matches!(
        capture_pomodoros::select_named(scan, query),
        capture_pomodoros::NamedSelection::Missing { .. }
    ) {
        return None;
    }
    pomodoro_creation_candidate(scan, query, plan_hint)
}

/// Theme count preview for a `creates_pomodoro` row: today's theme
/// count plus one for the new name, with the configured cap. `None`
/// when the ledger or the plan config is unavailable.
struct PlanCreationHint {
    before: usize,
    keys: Vec<String>,
    exempt: Vec<String>,
    cap: u32,
}

fn plan_creation_hint(
    contents: &str,
    scan: &capture_pomodoros::PomodoroScan,
    daily_file: Option<&str>,
) -> Option<PlanCreationHint> {
    if !scan.has_section {
        return None;
    }
    let config = config::load_plan_config(&config::config_path()).ok()?;
    let ledger = plan_budget::compute_for_daily(contents, &config, daily_file);
    Some(PlanCreationHint {
        before: ledger.themes.count,
        keys: ledger
            .theme_names
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect(),
        exempt: config
            .exempt()
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect(),
        cap: config.max_themes(),
    })
}

fn plan_themes_after_for(
    hint: Option<&PlanCreationHint>,
    name: &str,
) -> (Option<usize>, Option<u32>) {
    let Some(hint) = hint else {
        return (None, None);
    };
    let mut fresh: Vec<String> = Vec::new();
    for component in plan_budget::split_components(name) {
        let key = plan_budget::normalize_component(&component);
        if hint.exempt.contains(&key) || hint.keys.contains(&key) {
            continue;
        }
        if !fresh.contains(&key) {
            fresh.push(key);
        }
    }
    (Some(hint.before + fresh.len()), Some(hint.cap))
}

#[cfg(test)]
fn pomodoro_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    pomodoro_name_candidates_from_scan_with_hint(scan, query, None)
}

fn pomodoro_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    let mut candidates =
        pomodoro_name_candidates_from_entries(&scan.entries, query);
    if let Some(creation) =
        pomodoro_creation_candidate(scan, query, plan_hint.as_ref())
    {
        insert_pomodoro_creation_candidate(&mut candidates, creation, query);
    }
    candidates
}

fn pomodoro_creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<&PlanCreationHint>,
) -> Option<PomodoroNameCandidate> {
    let name = capture_pomodoros::named_creation_name(scan, query)?;
    let (plan_themes_after, plan_themes_cap) =
        plan_themes_after_for(plan_hint, &name);
    Some(PomodoroNameCandidate {
        replacement: capture_language::selector_slug(&name),
        pomodoro_ref: None,
        name: Some(name),
        requires_name: false,
        creates_pomodoro: true,
        next_up: false,
        plan_themes_after,
        plan_themes_cap,
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
    pomodoro_name_candidate_with_next_up(
        entry,
        requires_name,
        match_count,
        false,
    )
}

/// [`pomodoro_name_candidate`] plus the start-aware `next_up` marker. The
/// shared `pomodoro_name` context always passes false so its JSON stays
/// byte-identical; only `pomodoro_start_name` candidates set it.
fn pomodoro_name_candidate_with_next_up(
    entry: &PomodoroEntry,
    requires_name: bool,
    match_count: usize,
    next_up: bool,
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
        next_up,
        plan_themes_after: None,
        plan_themes_cap: None,
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
    if let Some(block_id) = &result.block_id {
        println!();
        println!("  {}", styler.dim(&block_id_summary(block_id)));
    }

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
    for line in candidate_lines(&result.candidates, result.context) {
        println!("    {} {}", styler.cyan(&line.0), styler.dim(&line.1));
    }
    print_warnings(result, styler);
    println!();
    println!("{} {}", result.candidates.len(), plural_candidates(result));
}

fn block_id_summary(block_id: &capture_block_ids::BlockIdField) -> String {
    let intent = match block_id.intent {
        capture_block_ids::BlockIdIntent::Link => "link",
        capture_block_ids::BlockIdIntent::New => "new",
        capture_block_ids::BlockIdIntent::ProjectNote => "project_note",
    };
    let count = block_id.used.len();
    let ids = if count == 1 { "ID" } else { "IDs" };
    let mut summary =
        format!("{} {intent} {count} {ids} in use", block_id.relative_target);
    if !block_id.suggestions.is_empty() {
        summary.push_str("  suggestions: ");
        summary.push_str(&block_id.suggestions.join(", "));
    }
    summary
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

/// Human row for one `task_link` candidate: the `@route:id` (or
/// `@route:…` for ID-less tasks) plus `[status] text  · tail`, where the
/// tail is the queued Pomodoro name (`Planned` when unnamed), `In
/// Progress`, `Next`, or the note label, with `· needs ID
/// (^suggestion)` on ID-less rows and `· scheduled DATE` whenever the task
/// carries a scheduled date.
fn task_link_line(item: &TaskLinkCandidate) -> (String, String) {
    let tail = match item.pomodoro.as_ref() {
        Some(pomodoro) => pomodoro
            .name
            .clone()
            .unwrap_or_else(|| "Planned".to_string()),
        None => match item.group {
            capture_link_tasks::LinkTaskGroup::Queued => "Planned".to_string(),
            capture_link_tasks::LinkTaskGroup::InProgress => {
                "In Progress".to_string()
            }
            capture_link_tasks::LinkTaskGroup::Next => "Next".to_string(),
            capture_link_tasks::LinkTaskGroup::Note => {
                format!("{}.md", item.route)
            }
        },
    };
    let mut detail =
        format!("[{}] {}  · {tail}", item.status_symbol, item.text);
    if item.requires_block_id {
        match item.block_id_suggestions.first() {
            Some(suggestion) => {
                detail.push_str(&format!(" · needs ID (^{suggestion})"))
            }
            None => detail.push_str(" · needs ID"),
        }
    }
    if let Some(scheduled) = &item.scheduled {
        detail.push_str(&format!("  · scheduled {scheduled}"));
    }
    let label = if item.requires_block_id {
        format!("@{}:…", item.route)
    } else {
        item.replacement.clone()
    };
    (label, detail)
}

fn candidate_lines(
    candidates: &Candidates,
    context: Option<CompletionContext>,
) -> Vec<(String, String)> {
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
                let detail = match item.pomodoro.as_ref() {
                    Some(pomodoro) => pomodoro.name.clone().map_or_else(
                        || format!("{}  · Planned", item.text),
                        |name| format!("{}  · {name}", item.text),
                    ),
                    None => item.text.clone(),
                };
                (label, detail)
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
        Candidates::ActiveTask(items) => items
            .iter()
            .map(|item| {
                let queue = match item.pomodoro.as_ref() {
                    Some(pomodoro) => pomodoro
                        .name
                        .clone()
                        .unwrap_or_else(|| "Planned".to_string()),
                    None => "Not queued".to_string(),
                };
                (
                    item.replacement.clone(),
                    format!(
                        "[{}] {}  · {}",
                        item.status_symbol, item.text, queue
                    ),
                )
            })
            .collect(),
        Candidates::TaskLink(items) => {
            items.iter().map(task_link_line).collect()
        }
        Candidates::PomodoroName(items) => items
            .iter()
            .map(|item| {
                if context == Some(CompletionContext::PomodoroStartName) {
                    return pomodoro_start_name_line(item);
                }
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

/// Human row for one `pomodoro_start_name` candidate: the visible name
/// plus the row-kind label (start/next-up, new, again, name-it, running)
/// with its link count or time range.
fn pomodoro_start_name_line(item: &PomodoroNameCandidate) -> (String, String) {
    let name = item.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let links = match item.child_count {
        0 => "Empty".to_string(),
        1 => "1 link".to_string(),
        count => format!("{count} links"),
    };
    let detail = if item.requires_name {
        format!("name it · {links}")
    } else if item.creates_pomodoro && item.state == PomodoroState::Completed {
        match item.time_range.as_deref() {
            Some(range) => format!("again · last {range}"),
            None => "again".to_string(),
        }
    } else if item.creates_pomodoro {
        "new session".to_string()
    } else if item.time_range.is_some() {
        format!("running {}", item.time_range.as_deref().unwrap_or(""))
    } else if item.next_up {
        format!("next up · {links}")
    } else {
        format!("planned · {links}")
    };
    (name, detail)
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
        CompletionContext::TaskBlockId => "task_block_id",
        CompletionContext::ProjectTaskBlockId => "project_task_block_id",
        CompletionContext::PomodoroName => "pomodoro_name",
        CompletionContext::PomodoroStartName => "pomodoro_start_name",
        CompletionContext::Task => "task",
        CompletionContext::TaskSection => "task_section",
        CompletionContext::ActiveTask => "active_task",
        CompletionContext::TaskLink => "task_link",
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

    /// Serializes the `BOB_DAY_FILE` override: the override is
    /// process-global, so parallel tests must never set and read it at the
    /// same time. Hold the guard for the whole body of any test that touches
    /// the day file.
    static DAY_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn day_file_guard() -> std::sync::MutexGuard<'static, ()> {
        DAY_FILE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

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

        // Marker-only `@route:` is link intent: identified linkable tasks.
        let value = result(temp.path(), "@Dev:foc", 8);
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
        assert!(tasks.iter().all(|task| task.line > 0));
        let block_id = value.block_id.as_ref().expect("block_id object");
        assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::Link);

        // The same marker on an item with text is new intent: no candidates.
        let with_text = result(temp.path(), "Do work @Dev:foc", 16);
        assert_eq!(with_text.context, Some(CompletionContext::PomodoroBlockId));
        assert_eq!(with_text.candidates.len(), 0);
        let block_id = with_text.block_id.as_ref().expect("block_id object");
        assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
    }

    fn active_task_fixture(root: &Path) -> PathBuf {
        write_settings(root);
        write_file(
            &root.join("sase.md"),
            concat!(
                "- [/] #task Outline talk ^outline\n",
                "- [*] #task Fix deep bug ^deep-fix\n",
                "- [ ] #task Ready thing ^ready\n",
            ),
        );
        let day_file = root.join("2026/20260710.md");
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () — BUGS\n  - [[sase#^deep-fix]]\n",
        );
        day_file
    }

    #[test]
    fn active_task_completion_offers_queued_tasks_first() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-active-task");
        let day_file = active_task_fixture(temp.path());

        let value =
            with_env("BOB_DAY_FILE", &day_file, || result(temp.path(), "^", 1));
        assert_eq!(value.context, Some(CompletionContext::ActiveTask));
        assert_eq!(value.replacement, Replacement { start: 1, end: 1 });
        let Candidates::ActiveTask(candidates) = &value.candidates else {
            panic!("expected active-task candidates");
        };
        // Ready tasks are excluded; the queued Next task sorts first.
        let replacements: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect();
        assert_eq!(replacements, vec!["sase:deep-fix", "sase:outline"]);

        let queued = &candidates[0];
        assert_eq!(queued.route, "sase");
        assert_eq!(queued.block_id, "deep-fix");
        assert_eq!(queued.status_symbol, '*');
        assert_eq!(queued.status_type, "ON_HOLD");
        assert_eq!(queued.text, "Fix deep bug");
        let pomodoro = queued.pomodoro.as_ref().expect("queued task");
        assert_eq!(pomodoro.name.as_deref(), Some("BUGS"));
        assert!(!pomodoro.is_current);

        let unqueued = &candidates[1];
        assert_eq!(unqueued.status_symbol, '/');
        assert!(unqueued.pomodoro.is_none());
        assert!(value.warnings.is_empty());
    }

    #[test]
    fn active_task_completion_ranks_queries_and_pins_json_shape() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-active-rank");
        let day_file = active_task_fixture(temp.path());

        let raw = "^sase:dee";
        let value = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });
        assert_eq!(value.context, Some(CompletionContext::ActiveTask));
        assert_eq!(value.replacement, Replacement { start: 1, end: 9 });
        let Candidates::ActiveTask(candidates) = &value.candidates else {
            panic!("expected active-task candidates");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].replacement, "sase:deep-fix");

        let json = serde_json::to_value(&value).expect("serialize result");
        assert_eq!(json["context"], "active_task");
        assert_eq!(
            json["replacement"],
            serde_json::json!({"start": 1, "end": 9})
        );
        let candidate = &json["candidates"][0];
        assert_eq!(candidate["replacement"], "sase:deep-fix");
        assert_eq!(candidate["ref"], candidates[0].task_ref);
        assert_eq!(candidate["route"], "sase");
        assert_eq!(candidate["block_id"], "deep-fix");
        assert_eq!(candidate["status_symbol"], "*");
        assert_eq!(candidate["text"], "Fix deep bug");
        assert_eq!(candidate["pomodoro"]["name"], "BUGS");
        assert_eq!(candidate["pomodoro"]["is_current"], false);
    }

    #[test]
    fn trailing_hash_fragment_requests_no_completion() {
        let temp = TempDir::new("bob-cli-capture-complete-now-tag");
        // `#now` is retired: a trailing `#n` is ordinary text with no
        // completion field, exactly like any other `#tag`.
        let raw = "Fix it #n";
        let value = result(temp.path(), raw, raw.len());
        assert_eq!(value.context, None);
        assert_eq!(value.candidates.len(), 0);
    }

    #[test]
    fn active_task_completion_excludes_ready_tasks() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-active-now");
        write_settings(temp.path());
        write_file(
            &temp.path().join("sase.md"),
            concat!(
                "- [*] #task Fix deep bug ^deep-fix\n",
                "- [ ] #task Ready bet #now ^ready-now\n",
                "- [ ] #task Ready thing ^ready\n",
            ),
        );
        let day_file = temp.path().join("2026/20260710.md");
        write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");

        let value =
            with_env("BOB_DAY_FILE", &day_file, || result(temp.path(), "^", 1));
        assert_eq!(value.context, Some(CompletionContext::ActiveTask));
        let Candidates::ActiveTask(candidates) = &value.candidates else {
            panic!("expected active-task candidates");
        };
        // Ready tasks stay excluded even with `#now` text; only the
        // Next task lists, with no `now` key in its JSON.
        let replacements: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect();
        assert_eq!(replacements, vec!["sase:deep-fix"]);

        let json = serde_json::to_value(&value).expect("serialize result");
        assert!(json["candidates"][0].get("now").is_none());
    }

    #[test]
    fn active_task_completion_keeps_suffixes_and_names_pomodoros() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-active-suffix");
        let day_file = active_task_fixture(temp.path());

        // The replacement always stops before `#`/`=`.
        let raw = "^sase:deep-fix#bu";
        let link = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, 14)
        });
        assert_eq!(link.context, Some(CompletionContext::ActiveTask));
        assert_eq!(link.replacement, Replacement { start: 1, end: 14 });

        // After `#` the same marker completes Pomodoro names.
        let name = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });
        assert_eq!(name.context, Some(CompletionContext::PomodoroName));
        let Candidates::PomodoroName(names) = &name.candidates else {
            panic!("expected Pomodoro-name candidates");
        };
        assert_eq!(names[0].replacement, "bugs");

        // Inside `=<X>` there is no completion field at all.
        let raw = "^sase:deep-fix=3";
        let empty = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });
        assert_eq!(empty.context, None);
        assert_eq!(empty.candidates.len(), 0);
    }

    #[test]
    fn close_items_and_suffixes_request_no_completion() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-close");
        let day_file = active_task_fixture(temp.path());
        // Whole-item `=x`/`=` are actions: empty success everywhere.
        for (raw, cursor) in [("=x", 2), ("=", 1), ("=x more", 3)] {
            let empty = with_env("BOB_DAY_FILE", &day_file, || {
                result(temp.path(), raw, cursor)
            });
            assert_eq!(empty.context, None, "{raw}");
            assert_eq!(empty.candidates.len(), 0, "{raw}");
        }
        // Inside a `=x` suffix there is no completion field.
        for raw in ["^sase:deep-fix=x", "Text @sase:deep-fix=x"] {
            let empty = with_env("BOB_DAY_FILE", &day_file, || {
                result(temp.path(), raw, raw.len())
            });
            assert_eq!(empty.context, None, "{raw}");
            assert_eq!(empty.candidates.len(), 0, "{raw}");
        }
        // Before the `=` the link still completes.
        let raw = "^sase:deep-fix=x";
        let link = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, 14)
        });
        assert_eq!(link.context, Some(CompletionContext::ActiveTask));
    }

    #[test]
    fn close_bullet_lines_suppress_wikilink_block_but_keep_note() {
        let temp = TempDir::new("bob-cli-capture-complete-close-bullet");
        write_file(&temp.path().join("Design notes.md"), "line ^web-capture\n");
        // Note completion keeps working on a Work Log bullet line.
        let note = result(temp.path(), "=x\n- 1 see [[Design", 17);
        assert_eq!(note.context, Some(CompletionContext::WikilinkNote));
        assert_ne!(note.candidates.len(), 0);
        // Note completion keeps working inside an inline entry too.
        let inline_note = result(temp.path(), "=x see [[Design", 15);
        assert_eq!(inline_note.context, Some(CompletionContext::WikilinkNote));
        assert_ne!(inline_note.candidates.len(), 0);
        // Block completion is suppressed on a Work Log bullet line, on a
        // plain close and on a chain close alike, and inside inline entries.
        for raw in [
            "=x\n- 1 see [[Design notes#^web",
            "=x =\n- 1 see [[Design notes#^web",
            "=x see [[Design notes#^web",
        ] {
            let block = result(temp.path(), raw, raw.len());
            assert_eq!(block.context, None, "{raw}");
            assert_eq!(block.candidates.len(), 0, "{raw}");
        }
        // Named-start completion on a trail token keeps working because the
        // owner's line ends at the entry.
        let raw = "=x wired it =#bu";
        let trail = result(temp.path(), raw, raw.len());
        assert_ne!(trail.context, None, "{raw}");
        // Marker completion (including `@@`) stays suppressed on bullet
        // lines and inline entries: entry text is literal.
        for raw in ["=x\n- 1 @@", "=x @@"] {
            let markers = result(temp.path(), raw, raw.len());
            assert_eq!(markers.context, None, "{raw}");
            assert_eq!(markers.candidates.len(), 0, "{raw}");
        }
    }

    #[test]
    fn active_task_human_rows_name_the_queue() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-active-human");
        let day_file = active_task_fixture(temp.path());

        let value =
            with_env("BOB_DAY_FILE", &day_file, || result(temp.path(), "^", 1));
        let rows = candidate_lines(&value.candidates, value.context);
        assert_eq!(
            rows,
            vec![
                (
                    "sase:deep-fix".to_string(),
                    "[*] Fix deep bug  · BUGS".to_string(),
                ),
                (
                    "sase:outline".to_string(),
                    "[/] Outline talk  · Not queued".to_string(),
                ),
            ]
        );
    }

    /// The `:` picker worked example: vault-wide linkable tasks in
    /// canonical order behind a `BOB_DAY_FILE` ledger, with the capture
    /// clock pinned so pull-forward flags are deterministic.
    fn task_link_fixture(root: &Path) -> PathBuf {
        write_file(
            &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"-","name":"Canceled","type":"CANCELLED"}
                ],
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
        write_file(
            &root.join("mac_inbox.md"),
            "---\ntype: [[area]]\n---\n- [ ] #task Call the bank [created::2026-09-29]\n",
        );
        write_file(
            &root.join("health.md"),
            "---\ntype: [[area]]\n---\n## Errands\n- [?] #task Book dentist [scheduled::2026-10-03]\n",
        );
        write_file(
            &root.join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Polish capture picker ^polish\n\t- [ ] #task Tune fuzzy weights\n",
        );
        write_file(
            &root.join("sase.md"),
            "---\ntype: [[project]]\nstatus: wip\n---\n## Bugs\n- [*] #task Fix deep bug ^deep-fix\n- [ ] #task Fix flaky gkeep test\n- [x] #task Old fix ^old-fix\n## Writing\n- [/] #task Draft outline ^outline\n- [ ] #task Ship blog post #now ^blog\n- [-] #task Dropped idea\n",
        );
        write_file(
            &root.join("archive.md"),
            "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Leftover ^leftover\n",
        );
        write_file(&root.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
        let day_file = root.join("2026/20260930.md");
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n",
        );
        day_file
    }

    fn task_link_result(
        root: &Path,
        day_file: &Path,
        raw: &str,
        cursor: usize,
    ) -> CaptureCompleteResult {
        with_env("BOB_DAY_FILE", day_file, || {
            with_env("BOB_NOW", "2026-09-30 09:02:00", || {
                result(root, raw, cursor)
            })
        })
    }

    #[test]
    fn task_link_completion_lists_worked_example_in_canonical_order() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link");
        let day_file = task_link_fixture(temp.path());

        let value = task_link_result(temp.path(), &day_file, ":", 1);
        assert_eq!(value.context, Some(CompletionContext::TaskLink));
        assert_eq!(value.replacement, Replacement { start: 0, end: 1 });
        let Candidates::TaskLink(candidates) = &value.candidates else {
            panic!("expected task-link candidates");
        };
        let replacements: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect();
        assert_eq!(
            replacements,
            vec![
                "@sase:deep-fix",
                "@sase:outline",
                "",
                "",
                "@bob:polish",
                "",
                "",
                "@sase:blog",
            ]
        );
        assert!(value.warnings.is_empty());
    }

    #[test]
    fn task_link_completion_pins_json_shape_and_omissions() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link-json");
        let day_file = task_link_fixture(temp.path());

        let value = task_link_result(temp.path(), &day_file, ":", 1);
        let json = serde_json::to_value(&value).expect("serialize result");
        assert_eq!(json["context"], "task_link");
        assert_eq!(json["schema_version"], 1);
        assert!(json.get("block_id").is_none());

        let identified = &json["candidates"][0];
        assert_eq!(
            identified,
            &serde_json::json!({
                "replacement": "@sase:deep-fix",
                "ref": identified["ref"],
                "route": "sase",
                "note_kind": "project",
                "block_id": "deep-fix",
                "requires_block_id": false,
                "block_id_suggestions": [],
                "status_symbol": "*",
                "status_name": "Next",
                "status_type": "ON_HOLD",
                "text": "Fix deep bug",
                "section": "Bugs",
                "depth": 0,
                "line": 6,
                "group": "queued",
                "scheduled": null,
                "pomodoro": {
                    "line": 2,
                    "name": "BUGS",
                    "time_range": null,
                    "is_current": false,
                },
            })
        );
        assert!(identified.get("now").is_none());
        assert!(identified.get("pulls_forward").is_none());

        let missing = &json["candidates"][6];
        assert_eq!(missing["replacement"], "");
        assert_eq!(missing["route"], "sase");
        assert_eq!(missing["note_kind"], "project");
        assert!(missing["block_id"].is_null());
        assert_eq!(missing["requires_block_id"], true);
        assert_eq!(
            missing["block_id_suggestions"],
            serde_json::json!(["fix-flaky-gkeep", "flaky-gkeep-test"])
        );
        assert_eq!(missing["status_symbol"], " ");
        assert_eq!(missing["text"], "Fix flaky gkeep test");
        assert_eq!(missing["section"], "Bugs");
        assert_eq!(missing["group"], "note");
        assert!(missing["scheduled"].is_null());
        assert!(missing["pomodoro"].is_null());
        assert!(missing.get("now").is_none());
        assert!(missing.get("pulls_forward").is_none());

        // The retired `#now` text stays on the row without a `now` key,
        // and the pull-forward flag serializes when set.
        let bet = &json["candidates"][7];
        assert_eq!(bet["replacement"], "@sase:blog");
        assert_eq!(bet["text"], "Ship blog post #now");
        assert_eq!(bet["group"], "note");
        assert!(bet.get("now").is_none());
        assert_eq!(json["candidates"][3]["pulls_forward"], true);
        assert_eq!(
            json["candidates"][3]["scheduled"],
            serde_json::json!("2026-10-03")
        );
    }

    #[test]
    fn task_link_completion_queries_cover_the_sigil_and_rank() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link-query");
        let day_file = task_link_fixture(temp.path());

        // `:dee` narrows to the queued row with a sigil-inclusive range.
        let raw = ":dee";
        let ranked = task_link_result(temp.path(), &day_file, raw, raw.len());
        assert_eq!(ranked.context, Some(CompletionContext::TaskLink));
        assert_eq!(ranked.replacement, Replacement { start: 0, end: 4 });
        let Candidates::TaskLink(candidates) = &ranked.candidates else {
            panic!("expected task-link candidates");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].replacement, "@sase:deep-fix");

        // A cursor just before the sigil refetches the full list.
        let full = task_link_result(temp.path(), &day_file, raw, 0);
        assert_eq!(full.context, Some(CompletionContext::TaskLink));
        assert_eq!(full.replacement, Replacement { start: 0, end: 4 });
        assert_eq!(full.candidates.len(), 8);
    }

    #[test]
    fn task_link_completion_scopes_to_the_batch_second_item() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link-batch");
        let day_file = task_link_fixture(temp.path());

        let raw = "Buy milk\n\n:dee";
        let value = task_link_result(temp.path(), &day_file, raw, raw.len());
        assert_eq!(value.context, Some(CompletionContext::TaskLink));
        assert_eq!(value.replacement, Replacement { start: 10, end: 14 });
        let Candidates::TaskLink(candidates) = &value.candidates else {
            panic!("expected task-link candidates");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].replacement, "@sase:deep-fix");
    }

    #[test]
    fn task_link_human_rows_name_queues_and_missing_ids() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link-human");
        let day_file = task_link_fixture(temp.path());

        let value = task_link_result(temp.path(), &day_file, ":", 1);
        let rows = candidate_lines(&value.candidates, value.context);
        assert_eq!(
            rows,
            vec![
                (
                    "@sase:deep-fix".to_string(),
                    "[*] Fix deep bug  · BUGS".to_string(),
                ),
                (
                    "@sase:outline".to_string(),
                    "[/] Draft outline  · In Progress".to_string(),
                ),
                (
                    "@mac_inbox:…".to_string(),
                    "[ ] Call the bank  · mac_inbox.md · needs ID (^call-bank)"
                        .to_string(),
                ),
                (
                    "@health:…".to_string(),
                    "[?] Book dentist  · health.md · needs ID (^book-dentist)  · scheduled 2026-10-03"
                        .to_string(),
                ),
                (
                    "@bob:polish".to_string(),
                    "[ ] Polish capture picker  · bob.md".to_string(),
                ),
                (
                    "@bob:…".to_string(),
                    "[ ] Tune fuzzy weights  · bob.md · needs ID (^tune-fuzzy-weights)"
                        .to_string(),
                ),
                (
                    "@sase:…".to_string(),
                    "[ ] Fix flaky gkeep test  · sase.md · needs ID (^fix-flaky-gkeep)"
                        .to_string(),
                ),
                (
                    "@sase:blog".to_string(),
                    "[ ] Ship blog post #now  · sase.md".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn task_link_completion_keeps_candidates_when_the_day_file_is_missing() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-link-warn");
        let day_file = task_link_fixture(temp.path());
        let missing =
            day_file.parent().expect("day parent").join("20990101.md");

        let value = task_link_result(temp.path(), &missing, ":", 1);
        assert_eq!(value.context, Some(CompletionContext::TaskLink));
        assert_eq!(value.candidates.len(), 8);
        assert!(
            value.warnings.iter().any(
                |warning| warning.contains("Bob daily note does not exist")
            ),
            "a missing ledger warns without dropping candidates: {:?}",
            value.warnings
        );
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
    fn hash_after_a_bare_block_id_marker_completes_a_pomodoro_name() {
        let _guard = day_file_guard();
        let temp = TempDir::new("bob-cli-capture-complete-task-toggle-hash");
        write_file(&temp.path().join("cash.md"), "- [ ] #task Parent ^bar\n");
        let day_file = temp.path().join("2026/20260828.md");
        write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n- [ ] ()\n");

        let raw = "@cash+bar#bu";
        let value = with_env("BOB_DAY_FILE", &day_file, || {
            result(temp.path(), raw, raw.len())
        });

        assert_eq!(value.context, Some(CompletionContext::PomodoroName));
        let Candidates::PomodoroName(candidates) = &value.candidates else {
            panic!("expected Pomodoro-name candidates");
        };
        assert_eq!(candidates[0].replacement, "bugs");

        // The same marker with body text keeps the task-section context.
        let with_body = "note @cash+bar#bu";
        let section = result(temp.path(), with_body, with_body.len());
        assert_eq!(section.context, Some(CompletionContext::TaskSection));
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
        let _guard = day_file_guard();
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
    fn pomodoro_name_completion_treats_plus_names_as_named_not_nameable() {
        let existing = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — C++\n",
            "- [ ] ()\n",
            "- [ ] () — SNAKE_CASE\n",
        ));
        let candidates = pomodoro_name_candidates_from_scan(&existing, "c+");
        assert_eq!(candidates[0].replacement, "c++");
        assert_eq!(candidates[0].name.as_deref(), Some("C++"));
        assert!(!candidates[0].requires_name);
        assert!(!candidates[0].creates_pomodoro);
        assert!(candidates.iter().any(|row| row.requires_name));
        assert!(candidates
            .iter()
            .filter(|row| row.replacement == "c++")
            .all(|row| !row.requires_name));

        let novel = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] () — MEMORY\n",
            "- [ ] ()\n",
        ));
        let created = pomodoro_name_candidates_from_scan(&novel, "c++");
        assert_eq!(created[0].replacement, "c++");
        assert_eq!(created[0].name.as_deref(), Some("C++"));
        assert!(created[0].creates_pomodoro);
        assert!(!created[0].requires_name);

        let infix = pomodoro_name_candidates_from_scan(&novel, "bob+sase");
        assert_eq!(infix[0].replacement, "bob+sase");
        assert_eq!(infix[0].name.as_deref(), Some("BOB+SASE"));
        assert!(infix[0].creates_pomodoro);
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

    /// The plan's worked-example ledger: a completed PLAN session, open
    /// BUGS and DEEP WORK placeholders, and one unnamed placeholder.
    fn named_start_ledger() -> capture_pomodoros::PomodoroScan {
        capture_pomodoros::scan(concat!(
            "# 2026-07-10\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "  - 🍅 [[sase#^plan-day]]\n",
            "- [ ] () — BUGS\n",
            "  - [[sase#^deep-fix]]\n",
            "- [ ] () — DEEP WORK\n",
            "  - [[bob#^outline]]\n",
            "  - [[bob#^draft]]\n",
            "- [ ] ()\n",
            "  - [[bob#^inbox-zero]]\n",
        ))
    }

    #[test]
    fn pomodoro_start_name_lists_start_again_and_name_it_rows() {
        let scan = named_start_ledger();
        let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
        let rows = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.replacement.as_str(),
                    candidate.name.as_deref(),
                    candidate.requires_name,
                    candidate.creates_pomodoro,
                    candidate.state,
                    candidate.time_range.as_deref(),
                    candidate.next_up,
                    candidate.child_count,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                (
                    "bugs",
                    Some("BUGS"),
                    false,
                    false,
                    PomodoroState::Open,
                    None,
                    true,
                    1
                ),
                (
                    "deep-work",
                    Some("DEEP WORK"),
                    false,
                    false,
                    PomodoroState::Open,
                    None,
                    false,
                    2
                ),
                (
                    "plan",
                    Some("PLAN"),
                    false,
                    true,
                    PomodoroState::Completed,
                    Some("0830-0855"),
                    false,
                    1
                ),
                ("", None, true, false, PomodoroState::Open, None, false, 1),
            ]
        );
        assert_eq!(candidates[0].line, Some(7));
        assert_eq!(candidates[1].line, Some(9));
        assert_eq!(candidates[2].line, Some(5));
        assert!(candidates[2].pomodoro_ref.is_some());
        assert!(candidates.iter().filter(|row| row.next_up).count() == 1);
        assert!(
            candidates.iter().all(|row| !row.creates_pomodoro
                || row.state == PomodoroState::Completed),
            "{candidates:?}"
        );
    }

    #[test]
    fn pomodoro_start_name_filters_and_creates_by_query() {
        let scan = named_start_ledger();

        let prefix = pomodoro_start_name_candidates_from_scan(&scan, "de");
        let prefix_rows = prefix
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect::<Vec<_>>();
        assert_eq!(prefix_rows, vec!["deep-work", ""]);

        let completed = pomodoro_start_name_candidates_from_scan(&scan, "pl");
        let completed_rows = completed
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect::<Vec<_>>();
        assert_eq!(completed_rows, vec!["plan", ""]);
        assert!(completed[0].creates_pomodoro);
        assert_eq!(completed[0].state, PomodoroState::Completed);

        let novel = pomodoro_start_name_candidates_from_scan(&scan, "rev");
        let novel_rows = novel
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect::<Vec<_>>();
        assert_eq!(novel_rows, vec!["rev", ""]);
        assert_eq!(novel[0].name.as_deref(), Some("REV"));
        assert!(novel[0].creates_pomodoro);
        assert!(novel[0].pomodoro_ref.is_none());
        assert!(novel[0].line.is_none());
        assert!(!novel[0].next_up);
    }

    #[test]
    fn pomodoro_start_name_again_rows_preview_plan_budget() {
        let scan = named_start_ledger();
        let hint = PlanCreationHint {
            before: 3,
            keys: vec![
                "bugs".to_string(),
                "deep work".to_string(),
                "goals".to_string(),
            ],
            exempt: vec!["gtd".to_string()],
            cap: 3,
        };
        let candidates = pomodoro_start_name_candidates_from_scan_with_hint(
            &scan,
            "",
            Some(hint),
        );
        let again = candidates
            .iter()
            .find(|row| row.replacement == "plan")
            .expect("again PLAN row");
        assert!(again.creates_pomodoro);
        assert_eq!(again.plan_themes_after, Some(4));
        assert_eq!(again.plan_themes_cap, Some(3));
        for slug in ["bugs", "deep-work"] {
            let row = candidates
                .iter()
                .find(|row| row.replacement == slug)
                .expect("start row");
            assert!(!row.creates_pomodoro, "{slug}");
            assert_eq!(row.plan_themes_after, None, "{slug}");
            assert_eq!(row.plan_themes_cap, None, "{slug}");
        }
        let name_it = candidates
            .iter()
            .find(|row| row.requires_name)
            .expect("name-it row");
        assert_eq!(name_it.plan_themes_after, None);
        assert_eq!(name_it.plan_themes_cap, None);

        let hint = PlanCreationHint {
            before: 3,
            keys: vec![
                "bugs".to_string(),
                "deep work".to_string(),
                "goals".to_string(),
            ],
            exempt: vec!["gtd".to_string()],
            cap: 3,
        };
        let novel = pomodoro_start_name_candidates_from_scan_with_hint(
            &scan,
            "fresh",
            Some(hint),
        );
        let created = novel
            .iter()
            .find(|row| row.replacement == "fresh")
            .expect("new FRESH row");
        assert!(created.creates_pomodoro);
        assert_eq!(created.plan_themes_after, Some(4));
        assert_eq!(created.plan_themes_cap, Some(3));

        let running_scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "- [ ] (**0840-0905** [t:: 25m]) — BUGS\n",
            "  - [[sase#^deep-fix]]\n",
            "- [ ] () — DEEP WORK\n",
            "  - [[bob#^outline]]\n",
            "- [ ] ()\n",
            "  - [[bob#^inbox-zero]]\n",
        ));
        let hint = PlanCreationHint {
            before: 3,
            keys: vec![
                "bugs".to_string(),
                "deep work".to_string(),
                "goals".to_string(),
            ],
            exempt: vec!["gtd".to_string()],
            cap: 3,
        };
        let running = pomodoro_start_name_candidates_from_scan_with_hint(
            &running_scan,
            "",
            Some(hint),
        );
        let timed = running
            .iter()
            .find(|row| {
                row.state == PomodoroState::Open && row.time_range.is_some()
            })
            .expect("running row");
        assert_eq!(timed.plan_themes_after, None);
        assert_eq!(timed.plan_themes_cap, None);
    }

    #[test]
    fn pomodoro_start_name_puts_the_running_entry_last() {
        let scan = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "- [ ] (**0840-0905** [t:: 25m]) — BUGS\n",
            "  - [[sase#^deep-fix]]\n",
            "- [ ] () — DEEP WORK\n",
            "  - [[bob#^outline]]\n",
            "  - [[bob#^draft]]\n",
            "- [ ] ()\n",
            "  - [[bob#^inbox-zero]]\n",
        ));
        let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
        let rows = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.replacement.as_str(),
                    candidate.name.as_deref(),
                    candidate.requires_name,
                    candidate.creates_pomodoro,
                    candidate.state,
                    candidate.time_range.as_deref(),
                    candidate.next_up,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                (
                    "deep-work",
                    Some("DEEP WORK"),
                    false,
                    false,
                    PomodoroState::Open,
                    None,
                    true
                ),
                (
                    "plan",
                    Some("PLAN"),
                    false,
                    true,
                    PomodoroState::Completed,
                    Some("0830-0855"),
                    false
                ),
                ("", None, true, false, PomodoroState::Open, None, false),
                (
                    "bugs",
                    Some("BUGS"),
                    false,
                    false,
                    PomodoroState::Open,
                    Some("0840-0905"),
                    false
                ),
            ]
        );
    }

    #[test]
    fn pomodoro_start_name_matches_pomodoro_name_warnings() {
        let temp = TempDir::new("bob-cli-capture-complete-start-name-warns");
        let missing_day = temp.path().join("2026/20260828.md");
        let (candidates, warnings) =
            pomodoro_start_name_candidates_at(&missing_day, "")
                .expect("warning success");
        assert_eq!(candidates.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("does not exist"));

        let sectionless = temp.path().join("2026/20260829.md");
        write_file(&sectionless, "# Day\n");
        let (candidates, warnings) =
            pomodoro_start_name_candidates_at(&sectionless, "")
                .expect("warning success");
        assert_eq!(candidates.len(), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no Pomodoros section"));

        // Several open timed entries leave no place for a new session.
        let ambiguous = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "- [ ] (1000-1030) — BUGS\n",
            "- [ ] ()\n",
        ));
        let candidates =
            pomodoro_start_name_candidates_from_scan(&ambiguous, "future");
        assert!(
            candidates.iter().all(|row| row.replacement != "future"),
            "{candidates:?}"
        );
    }

    #[test]
    fn pomodoro_start_name_human_labels_cover_every_row_kind() {
        let scan = named_start_ledger();
        let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
        assert_eq!(
            candidate_lines(
                &Candidates::PomodoroName(candidates),
                Some(CompletionContext::PomodoroStartName),
            ),
            vec![
                ("BUGS".to_string(), "next up · 1 link".to_string()),
                ("DEEP WORK".to_string(), "planned · 2 links".to_string()),
                ("PLAN".to_string(), "again · last 0830-0855".to_string()),
                ("unnamed".to_string(), "name it · 1 link".to_string()),
            ]
        );

        let novel = pomodoro_start_name_candidates_from_scan(&scan, "rev");
        assert_eq!(
            candidate_lines(
                &Candidates::PomodoroName(novel),
                Some(CompletionContext::PomodoroStartName),
            )[0],
            ("REV".to_string(), "new session".to_string()),
        );

        let running = capture_pomodoros::scan(concat!(
            "## Pomodoros\n",
            "- [ ] (0840-0905) — BUGS\n",
            "- [ ] () — DEEP WORK\n",
            "- [ ] ()\n",
        ));
        let running_candidates =
            pomodoro_start_name_candidates_from_scan(&running, "");
        let lines = candidate_lines(
            &Candidates::PomodoroName(running_candidates),
            Some(CompletionContext::PomodoroStartName),
        );
        assert_eq!(
            lines.last().expect("running row"),
            &("BUGS".to_string(), "running 0840-0905".to_string()),
        );
    }

    #[test]
    fn pomodoro_name_candidates_omit_next_up() {
        // The shared `pomodoro_name` context never sets `next_up`, so its
        // JSON stays byte-identical now that the field exists.
        let scan = named_start_ledger();
        let candidates = pomodoro_name_candidates_from_scan(&scan, "");
        let json = serde_json::to_string(&candidates).expect("json");
        assert!(!json.contains("next_up"), "{json}");
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

        // Link intent stays identified-only even with `--all-tasks`.
        let value = result_all(temp.path(), "@Dev:foc", 8);
        assert_eq!(value.context, Some(CompletionContext::PomodoroBlockId));
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].block_id.as_deref(), Some("focus-123"));
        assert!(!tasks[0].requires_block_id);

        // New intent stays empty even with `--all-tasks`.
        let with_text = result_all(temp.path(), "Do work @Dev:foc", 16);
        assert_eq!(with_text.candidates.len(), 0);
    }

    #[test]
    fn task_completion_before_an_explicit_toggle_bang_does_not_replace_the_bang(
    ) {
        let temp = TempDir::new("bob-cli-capture-complete-explicit-toggle");
        write_settings(temp.path());
        write_file(
            &temp.path().join("file.md"),
            "- [ ] #task Ready one ^ready-one\n",
        );
        let raw = "@file+ready!";
        let bang = raw.find('!').expect("bang");
        let value = result(temp.path(), raw, bang);
        assert_eq!(value.context, Some(CompletionContext::Task));
        assert_eq!(value.replacement.start, raw.find('+').expect("plus") + 1);
        assert_eq!(value.replacement.end, bang);
        let Candidates::Task(tasks) = &value.candidates else {
            panic!("expected task candidates");
        };
        assert_eq!(tasks[0].block_id.as_deref(), Some("ready-one"));
        assert_eq!(tasks[0].replacement, "ready-one");

        let after_bang = result(temp.path(), raw, raw.len());
        assert_eq!(after_bang.context, None);
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

        // The right-hand side of `@route^` is now a `task_block_id`
        // completion with empty candidates and a `new`-intent block object.
        let id_side = result(temp.path(), "Do @dev^new-id", 14);
        assert_eq!(id_side.context, Some(CompletionContext::TaskBlockId));
        assert_eq!(id_side.candidates.len(), 0);
        let block_id = id_side.block_id.as_ref().expect("block_id object");
        assert_eq!(block_id.route, "dev");
        assert_eq!(block_id.marker, "^");
        assert_eq!(block_id.intent, capture_block_ids::BlockIdIntent::New);
        assert_eq!(block_id.allowed_character, "[A-Za-z0-9-]");
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
            block_id: None,
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
            block_id: None,
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
                block_id: None,
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

        let lines =
            candidate_lines(&candidates, Some(CompletionContext::PomodoroName));

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
        let lines =
            candidate_lines(&candidates, Some(CompletionContext::PomodoroName));

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
            block_id: None,
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

    #[test]
    fn plan_themes_after_counts_only_fresh_non_exempt_components() {
        let hint = PlanCreationHint {
            before: 2,
            keys: vec!["a".to_string(), "goals".to_string()],
            exempt: vec!["gtd".to_string()],
            cap: 3,
        };
        assert_eq!(
            plan_themes_after_for(Some(&hint), "GTD"),
            (Some(2), Some(3))
        );
        assert_eq!(
            plan_themes_after_for(Some(&hint), "A + B"),
            (Some(3), Some(3))
        );
        assert_eq!(
            plan_themes_after_for(Some(&hint), "B + C"),
            (Some(4), Some(3))
        );
        assert_eq!(plan_themes_after_for(None, "B"), (None, None));

        let scan = capture_pomodoros::scan("## Pomodoros\n- [ ] () — A\n");
        for candidates in [
            pomodoro_name_candidates_from_scan_with_hint(
                &scan,
                "gtd",
                Some(PlanCreationHint {
                    before: 2,
                    keys: vec!["a".to_string(), "goals".to_string()],
                    exempt: vec!["gtd".to_string()],
                    cap: 3,
                }),
            ),
            pomodoro_start_name_candidates_from_scan_with_hint(
                &scan,
                "gtd",
                Some(PlanCreationHint {
                    before: 2,
                    keys: vec!["a".to_string(), "goals".to_string()],
                    exempt: vec!["gtd".to_string()],
                    cap: 3,
                }),
            ),
        ] {
            let created = candidates
                .iter()
                .find(|row| row.creates_pomodoro)
                .expect("creation row");
            assert_eq!(created.plan_themes_after, Some(2));
        }
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
