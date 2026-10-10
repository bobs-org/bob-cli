use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    iter,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture_language::{
        self, AuthoredSubBullet, DependencyEntry, DependencyTarget, Diagnostic,
        EditorGlobalDestination, EditorItemParse, EditorMode,
        EditorParseOptions, Need, PomodoroAdjustSpec, PomodoroCloseSpec,
        PomodoroShiftSpec, PomodoroStartSpec, ProjectTaskId, Severity, Span,
    },
    capture_links,
    style::Styler,
    url_routing::UrlRoutingPolicy,
};

const COMMAND_NAME: &str = "bob capture-parse";

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
    let raw_text = match raw_text_from_matches(&matches) {
        Ok(raw_text) => raw_text,
        Err(error) => return print_parse_error(&error, output_format),
    };
    if capture_language::normalize_task_text(&raw_text).is_empty() {
        return print_parse_error(
            &capture_language::missing_text_error(),
            output_format,
        );
    }

    // A config error stays silent here and keeps parse JSON
    // unchanged: routing turns off and the URL stays a task.
    let no_ref = matches.get_flag("no-ref");
    let policy = load_parse_routing(no_ref);
    let options = EditorParseOptions {
        url_routing: policy.as_ref(),
        has_global_destination: false,
    };
    let result = CaptureParseResult::new(raw_text, &options);
    print_success(&result, output_format);
    0
}

/// Load the URL routing policy for `capture-parse`. `-R` disables
/// routing; a config error or a `capture: false` toggle does too.
fn load_parse_routing(no_ref: bool) -> Option<UrlRoutingPolicy> {
    if no_ref {
        return None;
    }
    UrlRoutingPolicy::load()
        .ok()
        .filter(|policy| policy.capture)
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
        .about("Explain what in-progress capture text currently means")
        .long_about(
            "Report the authoritative capture grammar's view of TEXT.\n\n\
The command is purely lexical and completely read-only: it never opens the \
vault, never reads the clipboard, never touches the filesystem, and takes no \
--bob-dir. Running it with a nonexistent BOB_DIR and a '%' clipboard marker \
still succeeds.\n\n\
It reports the normalized body, the overall capture mode, the resolved route, \
section, and block ID, which parts a picker still has to supply, an optional \
global_destination object for a @@<route> or @@<route>+<block-id> \
declaration token anywhere in the draft, ordered blank-line-separated item summaries and ranges after \
inheritance, the UTF-8 byte spans of every recognized token, Obsidian wikilink \
component spans, every authored sub-bullet's normalized body plus depth, \
an optional pomodoro_start object for a `@<route>:<block-id>[#<name>]=<X>` \
start suffix, an optional pomodoro_adjust object for a whole-item `+[N]`/`-[N]` \
adjustment, an optional pomodoro_shift object for a whole-item `++[N]`/`--[N]` \
shift, an optional pomodoro_close object for a `=x` close suffix or \
whole-item close, and structured diagnostics. \
Wikilink highlighting is syntax-only and never touches the vault. Byte \
offsets index the original TEXT before whitespace normalization, are \
half-open [start, end), never overlap, and always land on a character \
boundary.\n\n\
TEXT accepts the same draft 'bob capture' does: one or more blank or \
whitespace-only physical lines separate capture items, and each item keeps \
the established authored-bullet grammar. A whitespace-separated line of \
only session operators yields one `items[]` entry per token, each `range` \
covers its token, and all entries share `line_start`/`line_end`. Within \
an item, the first physical \
line is the parent, later column-zero '-'/'*'/'+' lines become first-level \
authored children, and later lines prefixed by exactly two ASCII spaces \
become nested authored children. Separator rows have no spans, diagnostics, \
or completion fields. Incomplete interactive \
markers are valid input, \
not errors, on any line: '@', '@#', '@#Ideas', '@route#', '@^', \
'@route^', '@+', '@route+', '@route+id#', '@:', '@route:', '@route:id#', \
'@route:#name', and the legacy '@!' aliases \
all report mode 'incomplete' plus what they still need. A valid `=<X>` start \
suffix on a `@<route>:<block-id>[#<name>]` marker reports mode \
'pomodoro_task' with a `pomodoro_start` object (`raw` plus 5-minute \
`duration_units`/`offset_units`) and a `pomodoro_start` span covering the \
`=` and `<X>` bytes; the Pomodoro-name span always ends before the `=`. An \
invalid suffix is an `invalid_pomodoro_start` diagnostic instead. A \
whole-item `+[N]`/`-[N]` adjustment (for example `+5` extends today's \
current timed Pomodoro by 25 minutes, `-` shortens it by 5 minutes) reports \
mode 'pomodoro_adjust' with a `pomodoro_adjust` object (`raw` plus sign and \
5-minute `units`, defaulting to 1 when the count is omitted) and a \
`pomodoro_adjust` span covering only the signed token; the parse stays \
purely lexical and never guesses current ledger times. A whole-item \
`++[N]`/`--[N]` shift (for example `++3` moves today's running Pomodoro 15 \
minutes later, `--` moves it 5 minutes earlier) reports mode \
'pomodoro_shift' with a `pomodoro_shift` object (`raw`, direction as \
`later`, and 5-minute `units` defaulting to 1) and a `pomodoro_shift` span \
covering only the operator token. A `@@` declaration still routes ordinary \
items in the same draft but never turns an operator into a task or changes \
its destination. Invalid standalone counts (`+0`, `++0`, overflow) and \
malformed operator-first items (extra text, markers, or child lines) report \
an `invalid_pomodoro_adjustment` (one sign) or `invalid_pomodoro_shift` \
(two signs) diagnostic with a useful range, while `bob capture` keeps its \
strict execution errors for the same text. Other nonmatching words retain \
normal task semantics: a bare sign run followed by more text (`- foo`, \
`-- aside`), a run longer than two or mixed (`+++`, `+-`), and mid-body \
tokens (`Plan +5`, `C++`) stay ordinary prose. An operator is an action and \
requests no route or task completion candidates. A whole-item `=`/`=<X>` \
start (for example `=` starts 25 minutes, `=3` starts 15 minutes, `=-2` \
starts 25 minutes with a 10-minute offset) reports mode 'pomodoro_start' \
with a `pomodoro_start` object (`raw` excludes `=`, plus 5-minute \
`duration_units`/`offset_units` and the additive `drop` list) and a \
`pomodoro_start` span covering the whole token (only `=<X>` when a `~<K>` \
drop part is present, with a `pomodoro_start_drop` span covering `~<K>` \
including the `~`); the human `start` line reads `=3#bugs~1,3 (15m, \
offset 0u · drop 1, 3)`. The `==` family (`==`, `==<X>`, `==[<X>]#name`, \
`…~<K>`) parses the same way with an additive `override: true` flag on the \
spec and the doubled sigil in the human line; a bare `==` with more text \
stays prose (Obsidian `==highlight==` text), and `==x` is never a close. \
The parse stays purely lexical and never guesses \
current ledger times. A counted or drop-carrying token with extra text, \
markers, or child lines (`=3 more`, `=3x`, `=~2 more`), an exact token \
with child lines, and an oversized suffix report 'pomodoro_start' plus an \
`invalid_pomodoro_start` diagnostic (the extra text, the child line, or \
the token for overflow), while `bob capture` keeps its strict execution \
errors for the same text. A trailing `~<K>` drop list (`=~2`, `=3~2,4`, \
`=3#bugs~1`) starts the next session without those queued Task Links; a \
dangling `~`/`,` (`=~`, `=~2,`) reports mode 'incomplete' needing \
`pomodoro_start_task` with the partial spec, the spans typed so far, and \
one `interactive_placeholder` span over the separator, and every malformed \
list (a `0`, a duplicate, a second `~`, a `!`, a `#name` after the list, a \
bad character, an oversized number, or a space inside the list, which gets \
the no-spaces hint) reports `invalid_pomodoro_start` on the precise range. \
A named whole-item start (`=#bugs`, `=3#bugs`) starts that Pomodoro: \
`section` carries the typed name with a `pomodoro_name` span over the name \
bytes only (the `#` is in no span), `=<X>#` with no name reports mode \
'incomplete' needing `pomodoro_name` with an `interactive_placeholder` \
span over `#` and the partial `pomodoro_start` spec, and `=x#name` reports \
mode 'pomodoro_close' with an `invalid_pomodoro_close` diagnostic over \
`#name`. Named overrides (`==#bugs`, `==3#bugs`) parse the same way with \
the `override` flag; `==#` needs `pomodoro_name` with the partial override \
spec, and `==#bugs=3` teaches `==3#bugs`. A whole-item \
`=x[<N>][*<P>][!<M>][~<K>]` close (case-insensitive `=X`, with `*`, `!`, and `~` in \
any order; `=*`/`=!` omit `x` before an initial `*`/`!`) reports mode 'pomodoro_close' with a `pomodoro_close` object \
(`raw` exactly as typed plus the additive `in_progress` list, null when no `<N>` was typed, \
the `park` list, the `complete` list, the `drop` list, the additive `park_all` and `complete_all` flags, and the `log` entries (`index`, \
`text`, `details`) in typed order (`index` is omitted for unnumbered bullets under a close without `<N>`/`*<P>` or with wildcard intent, which `bob capture` resolves after staging the running session lineup); and spans covering the `=`/`=x` token (`pomodoro_close`), the `<N>` list \
including its commas (`pomodoro_close_in_progress`), the `*<P>` list \
including the `*` (`pomodoro_close_park`), the `!<M>` list \
including the `!` (`pomodoro_close_complete`), the `~<K>` list including \
the `~` (`pomodoro_close_drop`), and each numbered entry index \
(`pomodoro_close_log_index`; positional entries get no index span, and entry text \
renders as neutral prose but keeps \
its wikilink spans); the human `close` line reads \
`=x1*2!3~4 (in progress 1 · parked 2 · complete 3 · drop 4 · defer the rest)`, with \
`in progress none` for `=x0`, `park all` for `=*`, `complete all` for `=!`, `park all · complete 2` for `=x*!2`, `in progress 1 · park all` for `=x1*`, and a bare `=x` for a plain close, and \
`log 2 'wired the lexer' (+1 detail)` for typed entries with details \
(`log 'wired the lexer'` with no number for entries resolved at execution). \
One entry may sit on the close line itself (`bob capture-parse -f json -- \
'=x wired it'`): it logs to the first task the close works, or to the \
leading number when one is typed. Under wildcard intent, the default index is \
omitted and Bob resolves it to the first eligible top-level worked Task Link. The item `body` stays the close token \
(`=x`), the spec `log` carries the entry with an index only when lexically resolved, an \
explicit number gets a `pomodoro_close_log_index` span on the close line \
while the default gets none, and entry text keeps its wikilink spans. A \
dangling inline number (`bob capture-parse -f json -- '=x 2'`) reports mode \
'incomplete' needing `pomodoro_close_log_text` with the partial spec and an \
`interactive_placeholder` span over the number instead of its index span. \
Several entries still use child bullets below the close (`- [<n>] <text>`, with \
two-space `  - <detail>` details nesting under their entry): bullets are numbered \
all or none, and unnumbered bullets log in order to the close's worked tasks \
(one worked task takes them all, otherwise bullet `i` logs to worked task `i`); \
a leading number is always a task number, so `- 2 bugs fixed` names task 2. Only the \
first token is an index and every backslash stays literal. A \
dangling bullet (`- 1`) reports mode 'incomplete' needing \
`pomodoro_close_log_text` with the partial spec (lists plus every complete \
entry with its details), the spans typed so far, and one \
`interactive_placeholder` span over each dangling number instead of its \
index span. A mixed-numbering bullet, too many unnumbered bullets, a close \
that works no task, a non-loggable index, a bad number, a \
block link, or a fence reports 'pomodoro_close' plus an \
`invalid_pomodoro_close` diagnostic on the precise range, and so does a bad \
inline entry (stray marker, misplaced operators, no-spaces join, Task Link \
ending, block link, fence, or mixing with bullets). \
Every malformed list (duplicates, explicit overlaps, competing empty wildcard groups such as `=*!`, a misplaced `0`, a second `!` \
or `~`, a bad character, an oversized number, or a space inside the lists, \
which gets the no-spaces hint) reports the same. A token ending in a \
dangling separator (`=x1,`, `=x~`, `=*1,`, `=x!2,`, `=*~`) reports \
mode 'incomplete' needing `pomodoro_close_task`, with the partial spec, the \
spans typed so far, and one `interactive_placeholder` span over the \
separator. A trailing `*`/`!` is a wildcard instead of dangling, so `=*`, `=!`, `=x*`, and `=x!` are valid. Two empty wildcard groups conflict and require numbers on at least one group. A same-line chain splits into one `items[]` entry per operator, \
with child lines attaching to the line's `=x` (the last `=x` when it closes \
twice); a chain whose `=x` is not last nests its item ranges, so they never \
partially overlap. Other `=`-prefixed tokens \
(`=xx`, `=xa`, `==`, `= foo`) and mid-body `=x` stay ordinary prose. On \
link items the `=x…` suffix spans the same four span kinds after the route \
and block-ID spans: `@r:id=x1!2` and `^r:id=x1` stay 'pomodoro_link' (or \
'pomodoro_task' with body text) and carry the spec, `^r:id=x1,` reports \
'incomplete' needing `pomodoro_close_task`, while `#name=x…`, \
`s:<N>`/`p:<N>`/`%` conflicts, project-note `=x`, and malformed lists \
report `invalid_pomodoro_close` on the conflicting component or the precise \
list range. A `@@` declaration never applies to close, `=`/`=<X>`, or \
`=<X>#` items, and neither is ever rewritten. \
A '@^id+' marker already carries the project-note intent: it reports mode \
'project_note' with a 'route' need until the \
route is typed. A '@:id+' marker is the retired project-note form and \
reports a `retired_project_note_marker` diagnostic. In a project-note item, a trailing \
` :id` / ` ^id` word on a first-level task bullet names that task: ` :id` additionally makes it Next and \
links it into the Pomodoro, reporting mode 'pomodoro_project_note' with a `project_task_link_marker` span over \
the `:` and a `project_task_block_id` span over the ID (` ^id` spans only the ID). Stripped bodies appear in \
`sub_bullets` with a parallel `sub_bullet_task_ids` array of `null` or `{\"block_id\", \"link\"}` entries, and \
`section` carries the marker's Pomodoro name. Misplaced, invalid, reserved, empty, or checkbox-carrying IDs report \
`misplaced_project_task_id` or `invalid_project_task_id`, a repeated ID reports `duplicate_project_task_id`, and a \
`#pomodoro` name with no ` :` task reports `unused_project_note_pomodoro`. A lone trailing `:` or `^` is an unfinished \
ID: mode 'incomplete' needing `block_id` with an `interactive_placeholder` span over the sigil and no diagnostic. \
Outside project-note items these words stay literal text. A solo '@route:block-id[#pomodoro][=<X>]' item links an existing task into \
today's Pomodoro ledger instead of creating one, and parses as \
'pomodoro_link' with the same spans and `pomodoro_start` object; anything \
else on the item is an `invalid_pomodoro_link` diagnostic. The '^' spelling \
is identical except the marker resolves to `active_task_route` and \
`active_task_block_id` spans: '^', '^fragment', and '^route:' parse as \
'incomplete' needing `active_task` with one `interactive_placeholder` span, \
'^route:block-id#' needs `pomodoro_name`, and near misses stay \
`pomodoro_link` with an `invalid_pomodoro_link` diagnostic. Anything else \
starting with '^' ('^_^', '^^', '^.') stays ordinary text, and '^' is only \
recognized as the first token of an item's first line. A single-token, \
single-line item starting with ':' is a task-picker query: it reports mode \
'incomplete' needing `task_link` with one `interactive_placeholder` span \
over the whole token (sigil included) and no diagnostics, and it is never \
captured. A terminal `+query` on an item parent line or eligible authored \
child line is an unresolved parent-task selector: it reports `incomplete` \
needing `task_parent` with a placeholder over the full token, and capture \
refuses it until a task is selected. An exact lone `+` remains the existing \
Pomodoro adjustment in parsing. An `&note:block-id` modifier names a prerequisite task link \
(`&projects/foo:bar`; quote notes with spaces: `&\"Shopping List\":bar`): \
one or more modifiers lead or trail the parent line (interleaving with \
destination, schedule, priority, and clipboard markers in either order) or \
close an authored child line, and each adds a per-item `dependencies` entry \
(`raw`, decoded `note` and `block_id`, plus its sigil-inclusive `range`) \
with `dependency_sigil`, `dependency_note`, and `dependency_block_id` \
spans. Body text keeps its established mode and gains a `dependency_target` \
(`new_task` for a task this capture would create); a bare `&note:block-id` \
with an explicit `@note+task-id` (or the narrow bare `@note:id` colon \
alias) reports mode 'task_dependency' with an `existing_task` target. An \
ownerless `&note:block-id` reports mode 'incomplete' needing \
`dependency_target`, and a partial `&`, `&query`, or `&note:` needs \
`task_dependency` with a placeholder span over the typed query. A mid-line \
`&` between prose words (`Research & Development`, `R&D`), an `&` inside \
`[[wikilinks]]` or `` `code` ``, and `\\&` (which leaves a visible `&...` in \
task text) stay literal. `bob capture` executes recognized modifiers \
through the staged batch writer (managed `DEPENDS ON` line plus derived \
field and status effects); a draft with prerequisites never captures \
without them. The retired \
'@route::...' spelling is a diagnostic directing users to '@route^...'; \
it is not an incomplete Pomodoro marker. A bare trailing '#' reports mode \
'pomodoro_note' with no route, section, or block ID and an empty 'needs' list; \
combining it with an '@route' marker, 's:<N>', or 'p:<N>' on the same item \
reports a 'pomodoro_note_conflict' diagnostic instead of failing outright. \
An invalid marker component, a malformed \
continuation line, an orphaned nested bullet, an item emptied by marker removal, or a duplicate \
item-wide marker across lines becomes a diagnostic, so live editors keep \
a usable parse while 'bob capture' keeps its strict execution errors.\n\n\
A second @@ declaration reports 'duplicate_global_destination' on every later \
declaration token while keeping the first declaration effective. An item that \
has both a local destination marker and the @@ declaration it owns reports a \
'global_destination_shadowed' warning because the local marker wins for that item.\n\n\
Complete and in-progress Obsidian wikilinks such as '[[note', '![[note]]', \
'[[note#Heading|Alias]]', and '[[#^block-id]]' add semantic delimiter, target, \
heading, block, and alias spans without changing capture routing.\n\n\
Only a missing TEXT or a bad flag is an error; every other input succeeds. \
If TEXT is omitted and stdin is piped, it reads the complete piped stdin \
stream.",
        )
        .after_help(
            "Examples:\n  bob capture-parse --format json -- 'Call bank @cash+'\n  bob capture-parse 'Call bank @Cash+'\n  bob capture-parse -f json -- 'jot idea @notes#Ideas'\n  bob capture-parse -f json -- 'Postgres 17 minimum @foo+bar#req'\n  bob capture-parse -f json -- '@cash+goog-exit'\n  bob capture-parse -f json -- '+5'\n  bob capture-parse -f json -- '-2'\n  bob capture-parse -f json -- '++3'\n  bob capture-parse -f json -- '--'\n  bob capture-parse -f json -- '+'\n  printf '++3\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  printf '+5\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  echo 'Do work @dev^focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123#' | bob capture-parse -f json\n  printf '@@foo\\nFirst task\\n\\nSecond task @bar\\n' | bob capture-parse -f json\n  printf 'Parent\\n- first child\\n\\nSecond @work\\n' | bob capture-parse\n  bob capture-parse -f json -- '=x'\n  bob capture-parse -f json -- '=x1,3!2'\n  bob capture-parse -f json -- '=x1~2'\n  bob capture-parse -f json -- '=x0!2'\n  printf '=x\\n- 1 wired the lexer\\n' | bob capture-parse -f json\n  printf '=x\\n- 1\\n' | bob capture-parse -f json\n  printf '=x =\\n- 1 wired it\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '='\n  bob capture-parse -f json -- '=3'\n  bob capture-parse -f json -- '=~2'\n  bob capture-parse -f json -- '=3#bugs~1'\n  bob capture-parse -f json -- '=x =~2'\n  bob capture-parse -f json -- '@r:id=x'\n  bob capture-parse -f json -- '^r:id=x1'\n  printf '=x\\n\\n=\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '+2 =x'\n  bob capture-parse -f json -- 'Buy Groceries! &foo:bar'\n  bob capture-parse -f json -- '&foo:bar @body+excercise'\n  bob capture-parse -f json -- '&foo:bar'\n  bob capture-parse -f json -- 'Buy Groceries! &fo'\n\nModes:\n  task, bullet, pomodoro_task, pomodoro_note, sub_bullet, task_toggle, project_note, pomodoro_project_note, pomodoro_adjust, pomodoro_shift, pomodoro_link, pomodoro_close, pomodoro_start, task_dependency, task_complete, ref, incomplete\n\nNeeds:\n  route, section, block_id, pomodoro_id, pomodoro_name, task, task_section, active_task, task_link, task_parent, pomodoro_close_task, pomodoro_close_log_text, pomodoro_start_task, task_dependency, dependency_target, task_complete",
        )
        .disable_help_flag(true)
        .arg(format_arg())
        .arg(help_arg())
        .arg(no_ref_arg())
        .arg(text_arg())
}

fn no_ref_arg() -> Arg {
    Arg::new("no-ref")
        .long("no-ref")
        .short('R')
        .action(ArgAction::SetTrue)
        .help("Keep bare links as tasks instead of reporting them as reference items")
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

/// Mirror `bob capture`'s convention: join every TEXT argument with spaces,
/// or read the complete piped stdin stream when TEXT is omitted, so a
/// multi-line authored-bullet draft survives a pipe exactly like it does as
/// a single argv value.
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
    Ok(text)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureParseResult {
    ok: bool,
    schema_version: u32,
    input: String,
    body: String,
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_parent: Option<RefParentParse>,
    spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    /// Normalized authored-child bodies (source list marker and capture
    /// markers already removed) of every valid later physical line, in
    /// source order. Additive to schema version 1: omitted when empty, so
    /// an ordinary single-line draft's JSON shape is unchanged. These are
    /// semantic parse bodies, not rendered Markdown -- they carry no
    /// target-selected indentation or `- ` marker, unlike `bob capture`'s
    /// own `sub_bullets` output field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullets: Vec<String>,
    /// Additive schema-version-1 field aligned one-to-one with
    /// `sub_bullets`; each entry is `1` for a first-level authored child or
    /// `2` for a nested authored child.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullet_depths: Vec<u8>,
    /// Additive schema-version-1 field aligned one-to-one with
    /// `sub_bullets`: each entry is `null` or
    /// `{"block_id": "...", "link": bool}` (`link` is `true` for ` :id`).
    /// Emitted only when at least one entry is non-null, so every older
    /// input keeps its exact JSON shape.
    #[serde(skip_serializing_if = "all_task_ids_none")]
    sub_bullet_task_ids: Vec<Option<ProjectTaskId>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<CaptureParseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    global_destination: Option<GlobalDestinationParse>,
    /// Validated additive `@<route>:<block-id>[#<name>]=<X>` start suffix:
    /// the raw `<X>` text plus its 5-minute duration/offset units. Omitted
    /// for every older marker shape, so schema version 1 is unchanged for
    /// inputs without a start suffix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_start: Option<PomodoroStartSpec>,
    /// Validated additive whole-item `+[N]`/`-[N]` adjustment spec: the
    /// typed signed token plus its sign and 5-minute unit count. Omitted
    /// for every older input, so schema version 1 is unchanged for inputs
    /// without an adjustment. Purely lexical and never guesses current
    /// ledger times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_adjust: Option<PomodoroAdjustSpec>,
    /// Validated additive whole-item `++[N]`/`--[N]` shift spec: the typed
    /// operator token plus its direction and 5-minute unit count. Omitted
    /// for every older input, so schema version 1 is unchanged for inputs
    /// without a shift. Purely lexical and never guesses current ledger
    /// times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_shift: Option<PomodoroShiftSpec>,
    /// Validated additive `=x` close suffix: the typed `x` text as `raw`.
    /// Omitted for every older input, so schema version 1 is unchanged
    /// for inputs without a close. Purely lexical and never guesses
    /// current ledger times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_close: Option<PomodoroCloseSpec>,
    /// Complete `&note:block-id` modifiers on the first item, in source
    /// order: each carries its raw text, decoded note and block ID, and
    /// its sigil-inclusive range. Omitted when the draft names none, so
    /// schema version 1 is unchanged for older inputs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<ParseDependency>,
    /// Ownership of the first item's modifiers (`new_task` for a task
    /// this capture would create, `existing_task` for an explicitly
    /// selected dependent). Omitted when no dependent is named.
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency_target: Option<DependencyTarget>,
    /// Whole-item `!note:block-id` completion token on the first item,
    /// mirroring how top-level `dependencies` is emitted. Omitted when
    /// the draft names none, so schema version 1 is unchanged for older
    /// inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    task_complete: Option<ParseTaskComplete>,
}

/// One complete `&note:block-id` modifier with its sigil-inclusive range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseDependency {
    raw: String,
    note: String,
    block_id: String,
    quoted: bool,
    range: SourceRange,
}

/// One whole-item `!note:block-id` completion token with its
/// sigil-inclusive range. Present only on `task_complete` items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseTaskComplete {
    raw: String,
    note: String,
    block_id: String,
    quoted: bool,
    range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GlobalDestinationParse {
    range: SourceRange,
    line: usize,
    mode: EditorMode,
    route: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
}

/// Additive `ref_parent` on `ref` items: the lexical parent token
/// (explicit route, inherited global route, or `mac_inbox`) plus its
/// source (`explicit`|`global`|`default`). Outside `needs` so a bare URL
/// stays submittable while the client knows to offer a parent picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RefParentParse {
    token: String,
    source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureParseItem {
    index: usize,
    range: SourceRange,
    line_start: usize,
    line_end: usize,
    body: String,
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_parent: Option<RefParentParse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullets: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sub_bullet_depths: Vec<u8>,
    #[serde(skip_serializing_if = "all_task_ids_none")]
    sub_bullet_task_ids: Vec<Option<ProjectTaskId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_start: Option<PomodoroStartSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_adjust: Option<PomodoroAdjustSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_shift: Option<PomodoroShiftSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_close: Option<PomodoroCloseSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<ParseDependency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency_target: Option<DependencyTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_complete: Option<ParseTaskComplete>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct SourceRange {
    start: usize,
    end: usize,
}

impl CaptureParseResult {
    fn new(input: String, options: &EditorParseOptions<'_>) -> Self {
        let parse = capture_language::parse_for_editor_with(&input, options);
        let spans =
            merge_spans(parse.spans, capture_links::wikilink_spans(&input));
        let items = parse_items(&parse.items);
        let ref_parent = ref_parent_for(
            parse.mode,
            parse.route.as_deref(),
            parse.items.first().map(|item| item.has_local_destination),
        );
        let sub_bullets = parse.sub_bullets;
        let sub_bullet_depths = sub_bullet_depths(&sub_bullets);
        let sub_bullet_task_ids = sub_bullet_task_ids(&sub_bullets);
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            input,
            body: parse.body,
            mode: parse.mode,
            route: parse.route,
            section: parse.section,
            block_id: parse.block_id,
            needs: parse.needs,
            ref_parent,
            spans,
            diagnostics: parse.diagnostics,
            sub_bullets: sub_bullet_bodies(&sub_bullets),
            sub_bullet_depths,
            sub_bullet_task_ids,
            items,
            global_destination: parse
                .global_destination
                .as_ref()
                .map(global_destination_parse),
            pomodoro_start: parse.pomodoro_start,
            pomodoro_adjust: parse.pomodoro_adjust,
            pomodoro_shift: parse.pomodoro_shift,
            pomodoro_close: parse.pomodoro_close,
            dependencies: parse_dependencies(&parse.dependencies),
            dependency_target: parse.dependency_target,
            task_complete: parse.task_complete.as_ref().map(|entry| {
                ParseTaskComplete {
                    raw: entry.raw.clone(),
                    note: entry.note.clone(),
                    block_id: entry.block_id.clone(),
                    quoted: entry.quoted,
                    range: SourceRange {
                        start: entry.start,
                        end: entry.end,
                    },
                }
            }),
        }
    }
}

fn parse_dependencies(entries: &[DependencyEntry]) -> Vec<ParseDependency> {
    entries
        .iter()
        .map(|entry| ParseDependency {
            raw: entry.raw.clone(),
            note: entry.note.clone(),
            block_id: entry.block_id.clone(),
            quoted: entry.quoted,
            range: SourceRange {
                start: entry.start,
                end: entry.end,
            },
        })
        .collect()
}

fn global_destination_parse(
    global: &EditorGlobalDestination,
) -> GlobalDestinationParse {
    GlobalDestinationParse {
        range: SourceRange {
            start: global.start,
            end: global.end,
        },
        line: global.line,
        mode: global.mode,
        route: global.route.clone(),
        block_id: global.block_id.clone(),
        needs: global.needs.clone(),
    }
}

fn ref_parent_for(
    mode: EditorMode,
    route: Option<&str>,
    has_local: Option<bool>,
) -> Option<RefParentParse> {
    if mode != EditorMode::Ref {
        return None;
    }
    match (route, has_local.unwrap_or(false)) {
        (Some(route), true) => Some(RefParentParse {
            token: route.to_string(),
            source: "explicit",
        }),
        (Some(route), false) => Some(RefParentParse {
            token: route.to_string(),
            source: "global",
        }),
        (None, _) => Some(RefParentParse {
            token: "mac_inbox".to_string(),
            source: "default",
        }),
    }
}

fn parse_items(items: &[EditorItemParse]) -> Vec<CaptureParseItem> {
    if items.len() <= 1 {
        return Vec::new();
    }
    items
        .iter()
        .map(|item| CaptureParseItem {
            index: item.index + 1,
            range: SourceRange {
                start: item.start,
                end: item.end,
            },
            line_start: item.line_start,
            line_end: item.line_end,
            body: item.body.clone(),
            mode: item.mode,
            route: item.route.clone(),
            section: item.section.clone(),
            block_id: item.block_id.clone(),
            needs: item.needs.clone(),
            ref_parent: ref_parent_for(
                item.mode,
                item.route.as_deref(),
                Some(item.has_local_destination),
            ),
            sub_bullets: sub_bullet_bodies(&item.sub_bullets),
            sub_bullet_depths: sub_bullet_depths(&item.sub_bullets),
            sub_bullet_task_ids: sub_bullet_task_ids(&item.sub_bullets),
            pomodoro_start: item.pomodoro_start.clone(),
            pomodoro_adjust: item.pomodoro_adjust.clone(),
            pomodoro_shift: item.pomodoro_shift.clone(),
            pomodoro_close: item.pomodoro_close.clone(),
            dependencies: parse_dependencies(&item.dependencies),
            dependency_target: item.dependency_target.clone(),
            task_complete: item.task_complete.as_ref().map(|entry| {
                ParseTaskComplete {
                    raw: entry.raw.clone(),
                    note: entry.note.clone(),
                    block_id: entry.block_id.clone(),
                    quoted: entry.quoted,
                    range: SourceRange {
                        start: entry.start,
                        end: entry.end,
                    },
                }
            }),
        })
        .collect()
}

fn sub_bullet_bodies(sub_bullets: &[AuthoredSubBullet]) -> Vec<String> {
    sub_bullets.iter().map(|item| item.body.clone()).collect()
}

fn sub_bullet_depths(sub_bullets: &[AuthoredSubBullet]) -> Vec<u8> {
    sub_bullets.iter().map(|item| item.depth.level()).collect()
}

fn format_dependency_target(target: &DependencyTarget) -> String {
    let mut summary = match target.kind {
        super::capture_language::DependencyTargetKind::NewTask => {
            "new task".to_string()
        }
        super::capture_language::DependencyTargetKind::ExistingTask => {
            "existing task".to_string()
        }
    };
    if let Some(route) = target.route.as_deref() {
        summary.push_str(" @");
        summary.push_str(route);
        if let Some(block_id) = target.block_id.as_deref() {
            summary.push('+');
            summary.push_str(block_id);
        }
    }
    if target.inherited {
        summary.push_str(" (inherited)");
    }
    summary
}

fn sub_bullet_task_ids(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<Option<ProjectTaskId>> {
    sub_bullets
        .iter()
        .map(|item| item.task_id.clone())
        .collect()
}

fn all_task_ids_none(ids: &[Option<ProjectTaskId>]) -> bool {
    ids.iter().all(Option::is_none)
}

fn merge_spans(
    mut capture_spans: Vec<Span>,
    link_spans: Vec<Span>,
) -> Vec<Span> {
    capture_spans.extend(link_spans);
    capture_spans.sort_by_key(|span| (span.start, span.end));
    let mut merged = Vec::new();
    for span in capture_spans {
        if merged
            .last()
            .is_some_and(|previous: &Span| previous.end > span.start)
        {
            continue;
        }
        merged.push(span);
    }
    merged
}

fn print_success(result: &CaptureParseResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureParseResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureParseResult,
    styler: &Styler,
) {
    println!(
        "Capture parse {} {}",
        styler.separator(),
        styler.cyan(result.mode.label())
    );
    println!();
    print_field(styler, "body", &result.body);
    if let Some(global) = &result.global_destination {
        let mut summary = global
            .route
            .as_deref()
            .unwrap_or("(incomplete)")
            .to_string();
        if let Some(block_id) = global.block_id.as_deref() {
            summary.push_str(" +");
            summary.push_str(block_id);
        }
        print_field(styler, "global", &summary);
    }
    if let Some(route) = result.route.as_deref() {
        print_field(styler, "route", route);
    }
    if let Some(section) = result.section.as_deref() {
        print_field(styler, "section", section);
    }
    if let Some(block_id) = result.block_id.as_deref() {
        print_field(styler, "block id", block_id);
    }
    if let Some(start) = result.pomodoro_start.as_ref() {
        // Only a whole-item named start (`=<X>#name`) folds its name
        // into the start line; a `@<route>:<block-id>[#<name>]=<X>`
        // marker keeps `section` on its own line.
        let section = (result.mode == EditorMode::PomodoroStart)
            .then_some(result.section.as_deref())
            .flatten();
        print_field(styler, "start", &format_pomodoro_start(start, section));
    }
    if let Some(adjust) = result.pomodoro_adjust.as_ref() {
        print_field(styler, "adjust", &format_pomodoro_adjust(adjust));
    }
    if let Some(shift) = result.pomodoro_shift.as_ref() {
        print_field(styler, "shift", &format_pomodoro_shift(shift));
    }
    if let Some(close) = result.pomodoro_close.as_ref() {
        print_field(styler, "close", &format_pomodoro_close(close));
    }
    if !result.needs.is_empty() {
        let needs = result
            .needs
            .iter()
            .map(|need| need.label())
            .collect::<Vec<_>>()
            .join(", ");
        print_field(styler, "needs", &needs);
    }
    if !result.dependencies.is_empty() {
        let dependencies = result
            .dependencies
            .iter()
            .map(|dependency| dependency.raw.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        print_field(styler, "dependencies", &dependencies);
    }
    if let Some(target) = result.dependency_target.as_ref() {
        print_field(
            styler,
            "dependency target",
            &format_dependency_target(target),
        );
    }
    if let Some(task_complete) = result.task_complete.as_ref() {
        print_field(styler, "task complete", &task_complete.raw);
    }

    if !result.sub_bullets.is_empty() {
        println!();
        println!("  Sub-bullets");
        for (index, sub_bullet) in result.sub_bullets.iter().enumerate() {
            let depth =
                result.sub_bullet_depths.get(index).copied().unwrap_or(1);
            let indentation = "  ".repeat(usize::from(depth.saturating_sub(1)));
            let task_id = result
                .sub_bullet_task_ids
                .get(index)
                .and_then(|task| task.as_ref())
                .map(|task| {
                    format!(
                        " {}{}",
                        if task.link { ':' } else { '^' },
                        task.block_id
                    )
                })
                .unwrap_or_default();
            println!("    {indentation}- {sub_bullet}{task_id}");
        }
    }

    if !result.spans.is_empty() {
        println!();
        println!("  Spans");
        for span in &result.spans {
            println!(
                "    {}  {}",
                styler.dim(&format!("{}-{}", span.start, span.end)),
                styler.cyan(span.kind.label())
            );
        }
    }

    if !result.diagnostics.is_empty() {
        println!();
        println!("  Diagnostics");
        for diagnostic in &result.diagnostics {
            let range = diagnostic
                .range
                .map(|(start, end)| format!("{start}-{end}"))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "    {} {}  {}  {}",
                styled_severity(styler, diagnostic.severity),
                styler.dim(&range),
                styler.cyan(diagnostic.code),
                diagnostic.message
            );
        }
    }
}

/// Render a validated whole-item start for human output: the whole
/// typed token (`=`/`==` plus `<X>`, plus `#name` for a named start plus
/// `~<K>` for a drop list) plus its resolved 5-minute duration and offset
/// units. An override keeps its doubled sigil.
fn format_pomodoro_start(
    start: &PomodoroStartSpec,
    section: Option<&str>,
) -> String {
    let sigil = if start.r#override { "==" } else { "=" };
    let mut token = match section {
        Some(name) => format!("{sigil}{}#{name}", start.raw),
        None => format!("{sigil}{}", start.raw),
    };
    if !start.drop.is_empty() {
        let compact = start
            .drop
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        token.push('~');
        token.push_str(&compact);
    }
    if start.drop.is_empty() {
        return format!(
            "{token} ({}m, offset {}u)",
            start.duration_units.saturating_mul(5),
            start.offset_units
        );
    }
    format!(
        "{token} ({}m, offset {}u · drop {})",
        start.duration_units.saturating_mul(5),
        start.offset_units,
        join_numbers(&start.drop)
    )
}

/// Render a validated whole-item `+[N]`/`-[N]` adjustment for human
/// output: the typed signed token plus its resolved 5-minute minutes and
/// units.
fn format_pomodoro_adjust(adjust: &PomodoroAdjustSpec) -> String {
    format!(
        "{} ({}m, {} {})",
        adjust.raw,
        adjust.units.saturating_mul(5),
        adjust.units,
        unit_noun(adjust.units)
    )
}

/// Render a validated whole-item `++[N]`/`--[N]` shift for human output:
/// the typed operator token plus its resolved 5-minute minutes, direction,
/// and units.
fn format_pomodoro_shift(shift: &PomodoroShiftSpec) -> String {
    format!(
        "{} ({}m {}, {} {})",
        shift.raw,
        shift.units.saturating_mul(5),
        if shift.later { "later" } else { "earlier" },
        shift.units,
        unit_noun(shift.units)
    )
}

fn unit_noun(units: u64) -> &'static str {
    if units == 1 {
        "unit"
    } else {
        "units"
    }
}

/// Render a validated `=x[<N>][*<P>][!<M>][~<K>]` close for human output: the
/// typed token plus the outcome summary. Plain `=x` prints alone; a typed
/// `<N>` list without a wildcard ends with `defer the rest`, and `=x0` reads
/// `in progress none`. Typed Work Log entries render as
/// `log 2 "wired the lexer"`, one per entry in typed order, each followed
/// by `(+N detail)`/`(+N details)` when it carries details. Entries whose
/// index resolves at execution render as `log "text"`, with no number.
fn format_pomodoro_close(close: &PomodoroCloseSpec) -> String {
    let base = if close.raw.starts_with('=') {
        close.raw.clone()
    } else {
        format!("={}", close.raw)
    };
    let log_part = if close.log.is_empty() {
        None
    } else {
        let entries = close
            .log
            .iter()
            .map(|entry| {
                let head = match entry.index {
                    Some(index) => format!("{index} {:?}", entry.text),
                    None => format!("{:?}", entry.text),
                };
                if entry.details.is_empty() {
                    head
                } else {
                    let noun = if entry.details.len() == 1 {
                        "detail"
                    } else {
                        "details"
                    };
                    format!("{head} (+{} {noun})", entry.details.len())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("log {entries}"))
    };
    // `park_all`/`complete_all` are lexical intent. They select the
    // remaining links after explicit assignments, so their human summary
    // must not imply an ordinary deferred remainder.
    let has_work_selection = close.in_progress.is_some()
        || !close.park.is_empty()
        || close.park_all
        || close.complete_all;
    if !has_work_selection {
        let mut parts = Vec::new();
        if !close.complete.is_empty() {
            parts.push(format!("complete {}", join_numbers(&close.complete)));
        }
        if !close.drop.is_empty() {
            parts.push(format!("drop {}", join_numbers(&close.drop)));
        }
        if let Some(log) = log_part {
            parts.push(log);
        }
        if parts.is_empty() {
            return base;
        }
        return format!("{base} ({})", parts.join(" · "));
    };
    let mut parts = Vec::new();
    if let Some(in_progress) = close.in_progress.as_ref() {
        if in_progress.is_empty() {
            // `=x0*2` records work via parking: do not claim
            // `in progress none` while a parked task is being worked.
            if close.park.is_empty() && !close.park_all && !close.complete_all {
                parts.push("in progress none".to_string());
            }
        } else {
            parts.push(format!("in progress {}", join_numbers(in_progress)));
        }
    }
    let has_explicit_exceptions = close
        .in_progress
        .as_ref()
        .is_some_and(|numbers| !numbers.is_empty())
        || !close.park.is_empty()
        || !close.complete.is_empty()
        || !close.drop.is_empty();
    if close.park_all {
        parts.push(if has_explicit_exceptions {
            "parked the remaining links".to_string()
        } else {
            "parked all".to_string()
        });
    } else if !close.park.is_empty() {
        parts.push(format!("parked {}", join_numbers(&close.park)));
    }
    if close.complete_all {
        parts.push(if has_explicit_exceptions {
            "complete the remaining links".to_string()
        } else {
            "complete all".to_string()
        });
    } else if !close.complete.is_empty() {
        parts.push(format!("complete {}", join_numbers(&close.complete)));
    }
    if !close.drop.is_empty() {
        parts.push(format!("drop {}", join_numbers(&close.drop)));
    }
    if let Some(log) = log_part {
        parts.push(log);
    }
    if !close.park_all && !close.complete_all {
        parts.push("defer the rest".to_string());
    }
    format!("{base} ({})", parts.join(" · "))
}

fn join_numbers(numbers: &[u32]) -> String {
    numbers
        .iter()
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_field(styler: &Styler, label: &str, value: &str) {
    let rendered = if value.is_empty() {
        styler.dim("(empty)")
    } else {
        value.to_string()
    };
    println!("  {}  {rendered}", styler.dim(&pad_label(label)));
}

fn pad_label(label: &str) -> String {
    format!("{label:<8}")
}

fn styled_severity(styler: &Styler, severity: Severity) -> String {
    match severity {
        Severity::Error => styler.red(severity.label()),
        Severity::Warning => styler.yellow(severity.label()),
        Severity::Info => styler.blue(severity.label()),
    }
}

fn success_json(result: &CaptureParseResult) -> String {
    serde_json::to_string(result).expect("serialize capture parse result")
}

fn print_parse_error(message: &str, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {message}"),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": message }))
        }
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> CaptureParseResult {
        CaptureParseResult::new(
            input.to_string(),
            &EditorParseOptions::routing_off(),
        )
    }

    fn json(input: &str) -> serde_json::Value {
        serde_json::from_str(&success_json(&parse(input))).expect("json")
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn cli_joins_text_arguments_with_spaces() {
        let matches = build_cli()
            .try_get_matches_from(vec![
                COMMAND_NAME,
                "--",
                "Call",
                "bank",
                "@Cash+",
            ])
            .expect("parse arguments");
        assert_eq!(
            raw_text_from_matches(&matches).expect("text"),
            "Call bank @Cash+"
        );
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Human);
    }

    #[test]
    fn cli_accepts_the_json_format_alias() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "json", "body"])
            .expect("parse arguments");
        assert_eq!(OutputFormat::from_matches(&matches), OutputFormat::Json);
    }

    #[test]
    fn cli_rejects_an_unknown_format() {
        let error = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "-f", "yaml", "body"])
            .expect_err("unknown format");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn cli_keeps_hyphenated_text_literal_like_bob_capture() {
        let matches = build_cli()
            .try_get_matches_from(vec![COMMAND_NAME, "--", "-x", "@dev+"])
            .expect("parse arguments");
        assert_eq!(raw_text_from_matches(&matches).expect("text"), "-x @dev+");
    }

    #[test]
    fn json_shape_is_stable() {
        let value = json("Call bank @Cash+");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input"], "Call bank @Cash+");
        assert_eq!(value["body"], "Call bank");
        assert_eq!(value["mode"], "incomplete");
        assert_eq!(value["route"], "cash");
        assert!(value["section"].is_null());
        assert!(value["block_id"].is_null());
        assert_eq!(value["needs"][0], "task");
        assert_eq!(value["spans"][0]["start"], 10);
        assert_eq!(value["spans"][0]["end"], 15);
        assert_eq!(value["spans"][0]["kind"], "sub_bullet_route");
        assert_eq!(value["spans"][1]["start"], 15);
        assert_eq!(value["spans"][1]["end"], 16);
        assert_eq!(value["spans"][1]["kind"], "interactive_placeholder");
        assert_eq!(value["diagnostics"].as_array().expect("array").len(), 0);
        assert!(value.get("items").is_none(), "{value}");
        assert!(value.get("global_destination").is_none(), "{value}");
    }

    #[test]
    fn json_reports_batch_items_without_bumping_schema() {
        let raw = "First @cash\n\nSecond @notes#Ideas";
        let value = json(raw);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["body"], "First");
        assert_eq!(value["route"], "cash");
        assert_eq!(value["items"].as_array().expect("items").len(), 2);
        assert_eq!(value["items"][0]["index"], 1);
        assert_eq!(
            value["items"][0]["range"],
            json!({ "start": 0, "end": raw.find("\n\n").unwrap() })
        );
        assert_eq!(value["items"][0]["line_start"], 1);
        assert_eq!(value["items"][0]["line_end"], 1);
        assert_eq!(value["items"][0]["body"], "First");
        assert_eq!(value["items"][0]["route"], "cash");
        assert_eq!(value["items"][1]["index"], 2);
        assert_eq!(
            value["items"][1]["range"],
            json!({ "start": raw.find("\n\n").unwrap() + 2, "end": raw.len() })
        );
        assert_eq!(value["items"][1]["line_start"], 3);
        assert_eq!(value["items"][1]["line_end"], 3);
        assert_eq!(value["items"][1]["body"], "Second");
        assert_eq!(value["items"][1]["mode"], "bullet");
        assert_eq!(value["items"][1]["route"], "notes");
        assert_eq!(value["items"][1]["section"], "Ideas");
    }

    #[test]
    fn json_reports_every_mode_and_marker_kind() {
        assert_eq!(json("buy milk")["mode"], "task");
        assert_eq!(json("jot idea @notes#Ideas")["mode"], "bullet");
        assert_eq!(json("do work @dev:focus-1")["mode"], "pomodoro_task");
        assert_eq!(json("do work @dev:focus-1#bugs")["mode"], "pomodoro_task");
        assert_eq!(json("do work @dev:focus-1#")["mode"], "incomplete");
        assert_eq!(json("note @dev+focus-1")["mode"], "sub_bullet");
        assert_eq!(json("note @dev+")["mode"], "incomplete");
        assert_eq!(json("note @dev^focus-1")["mode"], "task");
        assert_eq!(json("note @dev^")["mode"], "incomplete");

        let value = json("body p:2 s:1 % @groceries");
        assert_eq!(value["body"], "body");
        assert_eq!(
            value["spans"]
                .as_array()
                .expect("array")
                .iter()
                .map(|span| span["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            vec!["priority", "schedule", "clipboard", "route"]
        );
    }

    #[test]
    fn json_reports_wikilink_semantic_spans_without_changing_capture_body() {
        let value = json("See [[sase#Design|Spec]] @Cash+");
        assert_eq!(value["body"], "See [[sase#Design|Spec]]");
        assert_eq!(value["route"], "cash");
        assert_eq!(
            value["spans"]
                .as_array()
                .expect("array")
                .iter()
                .map(|span| span["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            vec![
                "wikilink_delimiter",
                "wikilink_target",
                "wikilink_delimiter",
                "wikilink_heading",
                "wikilink_delimiter",
                "wikilink_alias",
                "wikilink_delimiter",
                "sub_bullet_route",
                "interactive_placeholder",
            ]
        );
    }

    #[test]
    fn json_ignores_wikilinks_inside_code_literals() {
        let value = json("Use `[[literal]]` and [[real]]");
        let kinds = value["spans"]
            .as_array()
            .expect("array")
            .iter()
            .map(|span| span["kind"].as_str().expect("kind"))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "wikilink_delimiter",
                "wikilink_target",
                "wikilink_delimiter",
            ]
        );
    }

    #[test]
    fn json_reports_pomodoro_name_spans_needs_and_diagnostics() {
        let complete = json("Do work @dev:id#bugs");
        assert_eq!(complete["mode"], "pomodoro_task");
        assert_eq!(complete["route"], "dev");
        assert_eq!(complete["block_id"], "id");
        assert_eq!(complete["section"], "bugs");
        assert_eq!(
            complete["spans"]
                .as_array()
                .expect("array")
                .iter()
                .map(|span| span["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            vec!["pomodoro_route", "pomodoro_block_id", "pomodoro_name"]
        );

        let incomplete = json("Do work @dev:id#");
        assert_eq!(incomplete["mode"], "incomplete");
        assert_eq!(incomplete["needs"][0], "pomodoro_name");
        assert_eq!(
            incomplete["spans"]
                .as_array()
                .expect("array")
                .last()
                .expect("span")["kind"],
            "interactive_placeholder"
        );

        let missing_id = json("Do work @dev:#bugs");
        assert_eq!(missing_id["mode"], "incomplete");
        assert_eq!(missing_id["needs"][0], "pomodoro_id");
        assert_eq!(missing_id["section"], "bugs");

        let invalid = json("Do work @dev:id#bad_id");
        assert_eq!(invalid["diagnostics"][0]["code"], "invalid_pomodoro_name");
        assert_eq!(invalid["mode"], "task");
    }

    #[test]
    fn json_reports_the_additive_start_suffix_without_bumping_schema() {
        let value = json("Do work @sase:outline#deep=-2");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["mode"], "pomodoro_task");
        assert_eq!(value["section"], "deep");
        assert_eq!(
            value["pomodoro_start"],
            serde_json::json!({
                "raw": "-2",
                "duration_units": 5,
                "offset_units": 2,
            })
        );
        let kinds = value["spans"]
            .as_array()
            .expect("array")
            .iter()
            .map(|span| span["kind"].as_str().expect("kind"))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "pomodoro_route",
                "pomodoro_block_id",
                "pomodoro_name",
                "pomodoro_start",
            ]
        );
        let spans = value["spans"].as_array().expect("array");
        assert_eq!(spans[2]["start"], 22);
        assert_eq!(spans[2]["end"], 26);
        assert_eq!(spans[3]["start"], 26);
        assert_eq!(spans[3]["end"], 29);

        let block_only = json("Do work @sase:outline=3");
        assert_eq!(
            block_only["pomodoro_start"],
            serde_json::json!({
                "raw": "3",
                "duration_units": 3,
                "offset_units": 0,
            })
        );

        // Older marker shapes keep the version-1 shape: no start field.
        let plain = json("Do work @dev:id#bugs");
        assert!(plain.get("pomodoro_start").is_none(), "{plain}");
        let incomplete = json("Do work @dev:id#");
        assert!(incomplete.get("pomodoro_start").is_none(), "{incomplete}");
    }

    #[test]
    fn json_reports_invalid_start_suffixes_as_diagnostics() {
        let value = json("Do work @sase:outline=abc");
        assert!(value.get("pomodoro_start").is_none(), "{value}");
        assert_eq!(value["diagnostics"][0]["code"], "invalid_pomodoro_start");

        let project_note = json("note @sase^outline+=3");
        assert!(
            project_note.get("pomodoro_start").is_none(),
            "{project_note}"
        );
        assert_eq!(
            project_note["diagnostics"][0]["code"],
            "invalid_project_note_marker"
        );
    }

    #[test]
    fn json_reports_per_item_start_suffixes_for_batches() {
        let raw = "First @sase:one=3\n\nSecond @sase:two#deep=-";
        let value = json(raw);
        assert_eq!(value["items"].as_array().expect("items").len(), 2);
        assert_eq!(
            value["items"][0]["pomodoro_start"],
            serde_json::json!({
                "raw": "3",
                "duration_units": 3,
                "offset_units": 0,
            })
        );
        assert_eq!(
            value["items"][1]["pomodoro_start"],
            serde_json::json!({
                "raw": "-",
                "duration_units": 5,
                "offset_units": 1,
            })
        );
        // The top-level preview still describes the first item.
        assert_eq!(
            value["pomodoro_start"],
            value["items"][0]["pomodoro_start"]
        );
    }

    #[test]
    fn json_reports_pomodoro_close_modes_spans_specs_and_diagnostics() {
        let value = json("=x");
        assert_eq!(value["mode"], "pomodoro_close");
        assert_eq!(value["body"], "=x");
        assert_eq!(value["pomodoro_close"]["raw"], "=x");
        assert_eq!(value["spans"][0]["kind"], "pomodoro_close");
        assert_eq!(value["spans"][0]["start"], 0);
        assert_eq!(value["spans"][0]["end"], 2);
        assert!(value["diagnostics"].as_array().expect("diags").is_empty());

        let upper = json("=X");
        assert_eq!(upper["mode"], "pomodoro_close");
        assert_eq!(upper["pomodoro_close"]["raw"], "=X");

        let link = json("@r:id=x");
        assert_eq!(link["mode"], "pomodoro_link");
        assert_eq!(link["pomodoro_close"]["raw"], "=x");
        assert!(link["diagnostics"].as_array().expect("diags").is_empty());

        let caret = json("^r:id=x");
        assert_eq!(caret["mode"], "pomodoro_link");
        assert_eq!(caret["pomodoro_close"]["raw"], "=x");

        let task = json("Text @r:id=x");
        assert_eq!(task["mode"], "pomodoro_task");
        assert_eq!(task["pomodoro_close"]["raw"], "=x");

        let shape = json("=x more");
        assert_eq!(shape["mode"], "pomodoro_close");
        assert_eq!(
            shape["pomodoro_close"]["log"],
            serde_json::json!([{ "index": 1, "text": "more" }])
        );
        assert_eq!(shape["body"], "=x");
        assert!(shape["diagnostics"].as_array().expect("diags").is_empty());

        let named = json("@r:id#n=x");
        assert_eq!(named["diagnostics"][0]["code"], "invalid_pomodoro_close");

        let scheduled = json("Text @r:id=x s:2");
        assert_eq!(
            scheduled["diagnostics"][0]["code"],
            "invalid_pomodoro_close"
        );

        for raw in ["Plan =x"] {
            let prose = json(raw);
            assert_eq!(prose["mode"], "task", "{raw}");
            assert!(prose.get("pomodoro_close").is_none(), "{raw}");
            assert!(prose.get("pomodoro_start").is_none(), "{raw}");
        }

        let raw = "+5\n\n=x\n\nCall bank @Cash+";
        let mixed = json(raw);
        assert_eq!(mixed["items"].as_array().expect("items").len(), 3);
        assert_eq!(mixed["items"][0]["mode"], "pomodoro_adjust");
        assert_eq!(mixed["items"][1]["mode"], "pomodoro_close");
        assert_eq!(mixed["items"][1]["pomodoro_close"]["raw"], "=x");
    }

    #[test]
    fn json_reports_named_pomodoro_starts() {
        let value = json("=#bugs");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["mode"], "pomodoro_start");
        assert_eq!(value["body"], "=#bugs");
        assert_eq!(value["section"], "bugs");
        assert_eq!(
            value["pomodoro_start"],
            serde_json::json!({
                "raw": "",
                "duration_units": 5,
                "offset_units": 0,
            })
        );
        assert_eq!(
            value["spans"],
            serde_json::json!([
                { "start": 0, "end": 1, "kind": "pomodoro_start" },
                { "start": 2, "end": 6, "kind": "pomodoro_name" },
            ])
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]));
        assert!(value.get("items").is_none(), "{value}");

        let counted = json("=3#bugs");
        assert_eq!(counted["mode"], "pomodoro_start");
        assert_eq!(counted["section"], "bugs");
        assert_eq!(
            counted["pomodoro_start"],
            serde_json::json!({
                "raw": "3",
                "duration_units": 3,
                "offset_units": 0,
            })
        );
        assert_eq!(
            counted["spans"],
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_start" },
                { "start": 3, "end": 7, "kind": "pomodoro_name" },
            ])
        );

        let incomplete = json("=#");
        assert_eq!(incomplete["mode"], "incomplete");
        assert_eq!(incomplete["needs"], serde_json::json!(["pomodoro_name"]));
        assert_eq!(
            incomplete["pomodoro_start"],
            serde_json::json!({
                "raw": "",
                "duration_units": 5,
                "offset_units": 0,
            })
        );
        assert_eq!(
            incomplete["spans"],
            serde_json::json!([
                { "start": 0, "end": 1, "kind": "pomodoro_start" },
                { "start": 1, "end": 2, "kind": "interactive_placeholder" },
            ])
        );
        assert_eq!(incomplete["diagnostics"], serde_json::json!([]));

        let order = json("=#bugs=3");
        assert_eq!(order["mode"], "pomodoro_start");
        assert!(order.get("pomodoro_start").is_none(), "{order}");
        assert_eq!(order["diagnostics"][0]["code"], "invalid_pomodoro_start");
        assert_eq!(order["diagnostics"][0]["range"], serde_json::json!([2, 8]));

        let multiword = json("=#deep work");
        assert_eq!(
            multiword["diagnostics"][0]["code"],
            "invalid_pomodoro_start"
        );
        assert_eq!(
            multiword["diagnostics"][0]["range"],
            serde_json::json!([7, 11])
        );

        let close_hash = json("=x#bugs");
        assert_eq!(close_hash["mode"], "pomodoro_close");
        assert_eq!(
            close_hash["diagnostics"][0]["code"],
            "invalid_pomodoro_close"
        );
        assert_eq!(
            close_hash["diagnostics"][0]["range"],
            serde_json::json!([2, 7])
        );
        assert_eq!(
            close_hash["spans"],
            serde_json::json!([
                { "start": 0, "end": 2, "kind": "pomodoro_close" },
            ])
        );

        let chain = json("=x =#bugs");
        assert_eq!(chain["mode"], "pomodoro_close");
        assert_eq!(chain["items"].as_array().expect("items").len(), 2);
        assert_eq!(chain["items"][0]["mode"], "pomodoro_close");
        assert_eq!(chain["items"][1]["mode"], "pomodoro_start");
        assert_eq!(chain["items"][1]["section"], "bugs");
        assert_eq!(chain["items"][1]["body"], "=#bugs");
        assert_eq!(
            chain["items"][1]["range"],
            serde_json::json!({ "start": 3, "end": 9 })
        );
    }

    #[test]
    fn json_reports_override_starts_with_the_doubled_sigil() {
        // An override keeps mode `pomodoro_start` with an additive
        // `"override": true` flag, the doubled sigil in the human line,
        // and spans covering `==<X>`.
        let value = json("==3#bugs");
        assert_eq!(value["mode"], "pomodoro_start");
        assert_eq!(value["body"], "==3#bugs");
        assert_eq!(value["section"], "bugs");
        assert_eq!(
            value["pomodoro_start"],
            serde_json::json!({
                "raw": "3",
                "duration_units": 3,
                "offset_units": 0,
                "override": true,
            })
        );
        assert_eq!(
            value["spans"],
            serde_json::json!([
                { "start": 0, "end": 3, "kind": "pomodoro_start" },
                { "start": 4, "end": 8, "kind": "pomodoro_name" },
            ])
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]));

        // A plain start omits the flag, so older clients read it unchanged.
        let plain = json("=3#bugs");
        assert_eq!(
            plain["pomodoro_start"],
            serde_json::json!({
                "raw": "3",
                "duration_units": 3,
                "offset_units": 0,
            })
        );

        // An override incomplete keeps the flag on its partial spec.
        let incomplete = json("==#");
        assert_eq!(incomplete["mode"], "incomplete");
        assert_eq!(incomplete["needs"], serde_json::json!(["pomodoro_name"]));
        assert_eq!(
            incomplete["pomodoro_start"],
            serde_json::json!({
                "raw": "",
                "duration_units": 5,
                "offset_units": 0,
                "override": true,
            })
        );
    }

    #[test]
    fn json_reports_diagnostics_with_a_range_pair() {
        let value = json("note @dev+bad.id");
        assert_eq!(value["ok"], true);
        assert_eq!(value["mode"], "task");
        assert_eq!(
            value["diagnostics"][0]["code"],
            "invalid_sub_bullet_block_id"
        );
        assert_eq!(value["diagnostics"][0]["severity"], "error");
        assert_eq!(value["diagnostics"][0]["range"][0], 5);
        assert_eq!(value["diagnostics"][0]["range"][1], 16);
    }

    #[test]
    fn json_reports_retired_double_colon_as_a_diagnostic() {
        let value = json("note @dev::new-id");
        assert_eq!(value["ok"], true);
        assert_eq!(value["mode"], "task");
        assert_eq!(
            value["diagnostics"][0]["code"],
            "retired_task_block_id_marker"
        );
        assert!(value["diagnostics"][0]["message"]
            .as_str()
            .expect("message")
            .contains("'@<route>::<block-id>' is no longer accepted"));
    }

    #[test]
    fn human_output_is_plain_without_color() {
        let styler = Styler::plain();
        assert!(!styler.is_color());
        // Rendering must not panic for a parse that exercises every branch.
        print_human_success_with_styler(
            &parse("note @dev+bad.id % s:1"),
            &styler,
        );
        print_human_success_with_styler(&parse("@"), &styler);
    }

    #[test]
    fn missing_text_uses_the_shared_capture_message() {
        assert_eq!(
            capture_language::missing_text_error(),
            "task text is required; pass TEXT or pipe it on stdin"
        );
    }

    #[test]
    fn spans_stay_ordered_and_on_character_boundaries() {
        let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit s:2";
        let result = parse(raw);
        assert!(!result.spans.is_empty());
        for span in &result.spans {
            assert!(raw.is_char_boundary(span.start));
            assert!(raw.is_char_boundary(span.end));
        }
        for pair in result.spans.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    #[test]
    fn json_reports_an_inherited_global_destination_without_bumping_schema() {
        let raw = "@@Foo\nFirst task\n\nSecond task @bar";
        let value = json(raw);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["body"], "First task");
        assert_eq!(value["mode"], "task");
        assert_eq!(value["route"], "foo");
        assert_eq!(value["global_destination"]["mode"], "task");
        assert_eq!(value["global_destination"]["route"], "foo");
        assert_eq!(value["global_destination"]["line"], 1);
        assert!(value["global_destination"]["block_id"].is_null());
        assert_eq!(
            value["global_destination"]["range"],
            json!({ "start": 0, "end": 5 })
        );
        assert_eq!(value["items"].as_array().expect("items").len(), 2);
        assert_eq!(value["items"][0]["index"], 1);
        assert_eq!(value["items"][0]["route"], "foo");
        assert_eq!(value["items"][0]["body"], "First task");
        assert_eq!(value["items"][1]["index"], 2);
        assert_eq!(value["items"][1]["route"], "bar");
        assert_eq!(value["spans"][0]["kind"], "global_route", "{value}");
    }

    #[test]
    fn json_reports_a_global_sub_bullet_declaration_and_local_override() {
        let raw = "@@foo+a-id\nNote one\n\nIndependent @bar";
        let value = json(raw);
        assert_eq!(value["mode"], "sub_bullet");
        assert_eq!(value["route"], "foo");
        assert_eq!(value["block_id"], "a-id");
        assert_eq!(value["global_destination"]["mode"], "sub_bullet");
        assert_eq!(value["global_destination"]["route"], "foo");
        assert_eq!(value["global_destination"]["block_id"], "a-id");
        assert_eq!(value["items"][0]["mode"], "sub_bullet");
        assert_eq!(value["items"][0]["block_id"], "a-id");
        assert_eq!(value["items"][1]["mode"], "task");
        assert_eq!(value["items"][1]["route"], "bar");
        assert!(value["items"][1]["block_id"].is_null());
    }

    #[test]
    fn format_pomodoro_close_reports_detail_counts() {
        use crate::native::capture_language::CloseLogEntry;
        let close = PomodoroCloseSpec {
            raw: "=x1,2".to_string(),
            in_progress: Some(vec![1, 2]),
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: vec![
                CloseLogEntry {
                    index: Some(1),
                    text: "wired the lexer".to_string(),
                    details: vec!["chose a hand-rolled lexer".to_string()],
                    origin: Default::default(),
                },
                CloseLogEntry {
                    index: Some(2),
                    text: "sketched the parser".to_string(),
                    details: vec!["a".to_string(), "b".to_string()],
                    origin: Default::default(),
                },
                CloseLogEntry {
                    index: Some(1),
                    text: "opened the PR".to_string(),
                    details: Vec::new(),
                    origin: Default::default(),
                },
            ],
        };
        assert_eq!(
            format_pomodoro_close(&close),
            "=x1,2 (in progress 1, 2 · log 1 \"wired the lexer\" (+1 detail), 2 \"sketched the parser\" (+2 details), 1 \"opened the PR\" · defer the rest)"
        );
        let plain = PomodoroCloseSpec::plain("=x".to_string());
        assert_eq!(format_pomodoro_close(&plain), "=x");
    }

    #[test]
    fn format_pomodoro_close_describes_wildcard_scope() {
        let bare_park = PomodoroCloseSpec {
            raw: "=*".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: true,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(format_pomodoro_close(&bare_park), "=* (parked all)");

        let park_except_complete = PomodoroCloseSpec {
            raw: "=*!2".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: true,
            complete: vec![2],
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(
            format_pomodoro_close(&park_except_complete),
            "=*!2 (parked the remaining links · complete 2)"
        );

        let progress_exception = PomodoroCloseSpec {
            raw: "=x1*".to_string(),
            in_progress: Some(vec![1]),
            park: Vec::new(),
            park_all: true,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(
            format_pomodoro_close(&progress_exception),
            "=x1* (in progress 1 · parked the remaining links)"
        );

        let bare_complete = PomodoroCloseSpec {
            raw: "=!".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: true,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(format_pomodoro_close(&bare_complete), "=! (complete all)");
    }

    #[test]
    fn json_reports_a_declaration_only_draft_as_a_diagnostic() {
        let value = json("@@foo");
        assert_eq!(value["ok"], true);
        assert_eq!(value["global_destination"]["route"], "foo");
        assert_eq!(value["diagnostics"][0]["code"], "missing_capture_item");
        assert!(value.get("items").is_none(), "{value}");
    }
}
