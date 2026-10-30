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
    capture_flag_guard,
    capture_language::{
        self, resolve_exact_type_name, scan_line, unmatched_type_display,
        AuthoredSubBullet, DependencyEntry, DependencyTarget, Diagnostic,
        EditorGlobalDestination, EditorItemParse, EditorMode,
        EditorParseOptions, Need, PomodoroAdjustSpec, PomodoroCloseSpec,
        PomodoroShiftSpec, PomodoroStartSpec, ProjectTaskId, RelationDirection,
        RelationEntry, ResolvedTypeName, Severity, Span,
    },
    capture_links,
    pomodoro_themes::{ThemeLookup as ThemeCatalog, ThemeRef},
    style::Styler,
    url_routing::UrlRoutingPolicy,
};

const COMMAND_NAME: &str = "bob capture-parse";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

/// Parse data a saved-template client needs to render a ready-to-insert
/// draft. This deliberately reuses capture-parse's template parser so the
/// CLI and clients share one source of syntax spans and route semantics.
pub(crate) struct TemplateProtocolParse {
    pub(crate) route: Option<String>,
    pub(crate) spans: Vec<CaptureParseSpan>,
}

pub(crate) fn parse_template_protocol(text: &str) -> TemplateProtocolParse {
    let prefix = "@@template:\n";
    let draft = format!("{prefix}{text}\n\n");
    let text_start = prefix.len();
    let text_end = text_start + text.len();
    let options = EditorParseOptions {
        url_routing: None,
        has_global_destination: false,
    };
    let parsed = CaptureParseResult::new(draft, &options);
    let route = parsed.items.first().and_then(|item| item.route.clone());
    let spans = parsed
        .spans
        .into_iter()
        .filter(|span| span.start >= text_start && span.end <= text_end)
        .map(|span| CaptureParseSpan {
            start: span.start - text_start,
            end: span.end - text_start,
            kind: span.kind,
            field: span.field,
            themes: span.themes,
            cancels_source: span.cancels_source,
            completes_source: span.completes_source,
        })
        .collect();
    TemplateProtocolParse { route, spans }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    if let Some(hit) = capture_flag_guard::find_unknown_long_flag(
        &args,
        &capture_flag_guard::capture_parse_spec(),
    ) {
        let message =
            capture_flag_guard::unexpected_argument_message(&hit.flag);
        if hit.format_json {
            return print_parse_error(&message, OutputFormat::Json);
        }
        return print_clap_error(
            command.error(clap::error::ErrorKind::UnknownArgument, message),
        );
    }
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
    let mut result = CaptureParseResult::new(raw_text, &options);
    let theme_keys = theme_component_keys(&result);
    if !theme_keys.is_empty() {
        let bob_dir = super::env::bob_dir();
        let day_file = super::pomodoro::day_file_for(&bob_dir);
        let today = super::capture_pomodoros_agenda::agenda_date(&day_file);
        if let Ok(catalog) =
            ThemeCatalog::load_for_theme_keys(&bob_dir, today, &theme_keys)
        {
            attach_theme_refs(&mut result, &catalog);
        }
    }
    for warning in load_close_policy().warnings {
        eprintln!("{COMMAND_NAME}: {warning}");
    }
    print_success(&result, output_format);
    0
}

/// Load the URL routing policy for `capture-parse`. `-R` disables
/// routing; a config error or a `capture: false` toggle does too.
pub(crate) fn load_parse_routing(no_ref: bool) -> Option<UrlRoutingPolicy> {
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
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Explain what in-progress capture text currently means")
        .long_about(
            "Report the authoritative capture grammar's view of TEXT.\n\n\
The grammar parse is lexical and read-only. For Pomodoro-name spans, the \
command also best-effort reads the configured vault's theme catalog and adds \
theme refs; if the catalog is unavailable, parsing still succeeds without \
those refs. It never writes to the vault or reads the clipboard, and takes no \
--bob-dir. A '%' clipboard marker still succeeds.\n\n\
It reports the normalized body, the overall capture mode, the resolved route, \
section, and block ID, which parts a picker still has to supply, an optional \
global_destination object for a @@<route> or @@<route>+<block-id> \
declaration token anywhere in the draft, ordered blank-line-separated item summaries and ranges after \
inheritance, the UTF-8 byte spans of every recognized token (including a \
`task_type` span for `t:<type>` markers), Obsidian wikilink \
component spans, every authored sub-bullet's normalized body plus depth, \
an optional pomodoro_start object for a `@<route>:<block-id>[#<name>]=<X>` \
start suffix (including the short `=name` suffix, e.g. `@work:outline=bugs`), an optional pomodoro_adjust object for a whole-item `+[N]`/`-[N]` \
adjustment, an optional pomodoro_shift object for a whole-item `++[N]`/`--[N]` \
shift, an optional pomodoro_close object for a `=x` link close suffix or \
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
offset 0u · drop 1, 3)`. Every claimed doubled start (`==`, `==3#bugs`, \
`==#`, `==~`, `==#bugs=3`) reports mode 'pomodoro_start' with an \
`invalid_pomodoro_start` diagnostic over the token and a message naming its \
single-`=` rewrite. The `==x` family reports that it is not a close and \
teaches the bare `x` rewrite. Bare `==` followed by prose stays ordinary \
text, preserving Obsidian highlights. \
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
A named whole-item start (`=bugs`, `=3#bugs`) starts that Pomodoro: \
`section` carries the typed name with a `pomodoro_name` span over the name \
bytes only (the `#` is in no span; `=bugs` emits `pomodoro_start [0,1)` and `pomodoro_name [1,5)`), `=<X>#` with no name reports mode \
'incomplete' needing `pomodoro_name` with an `interactive_placeholder` \
span over `#` and the partial `pomodoro_start` spec, and `x#name` reports \
mode 'pomodoro_close' with an `invalid_pomodoro_close` diagnostic over \
`#name`. A trailing `@<X>` timing (`=bugs@3` means `=3#bugs`, \
`=~2@3` means `=3~2`) reports the canonical spec with a second \
`pomodoro_start` span over `@<X>`; a dangling `@` reports `incomplete` \
needing `pomodoro_start_timing` with no spec. A whole-item \
`x[<N>][*<P>][!<M>][~<K>][@<N>]` close (lowercase `x` only, with `*`, `!`, `~`, and `@` in \
any order; a capitalized `X…` stays prose) reports mode 'pomodoro_close' with a `pomodoro_close` object \
(`raw` exactly as typed plus the additive `in_progress` list, null when no `<N>` was typed, \
the `park` list, the `complete` list, the `drop` list, the additive `park_all` and `complete_all` flags, and the `log` entries (`index`, \
`text`, `details`) in typed order (`index` is omitted for unnumbered bullets under a close without `<N>`/`*<P>` or with wildcard intent, which `bob capture` resolves after staging the running session lineup); and spans covering the `x` token (`pomodoro_close`), the `<N>` list \
including its commas (`pomodoro_close_in_progress`), the `*<P>` list \
including the `*` (`pomodoro_close_park`), the `!<M>` list \
including the `!` (`pomodoro_close_complete`), the `~<K>` list including \
the `~` (`pomodoro_close_drop`), and each numbered entry index \
(`pomodoro_close_log_index`; positional entries get no index span, and entry text \
renders as neutral prose but keeps \
its wikilink spans); the human `close` line reads \
`x1*2!3~4 (in progress 1 · parked 2 · complete 3 · drop 4 · defer the rest)`, with \
`in progress none` for `x0`, `park all` for `x*`, `complete all` for `x!`, `park all · complete 2` for `x*!2`, `in progress 1 · park all` for `x1*`, and a bare `x` for a plain close, and \
`log 2 'wired the lexer' (+1 detail)` for typed entries with details \
(`log 'wired the lexer'` with no number for entries resolved at execution). \
One entry may sit on the close line itself (`bob capture-parse -f json -- \
'x wired it'`): it logs to the first task the close works, or to the \
leading number when one is typed. Under wildcard intent, the default index is \
omitted and Bob resolves it to the first eligible top-level worked Task Link. The item `body` stays the close token \
(`x`), the spec `log` carries the entry with an index only when lexically resolved, an \
explicit number gets a `pomodoro_close_log_index` span on the close line \
while the default gets none, and entry text keeps its wikilink spans. A \
dangling inline number (`bob capture-parse -f json -- 'x 2'`) reports mode \
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
Every malformed list (duplicates, explicit overlaps, competing empty wildcard groups such as `x*!`, a misplaced `0`, a second `!` \
or `~`, a bad character, an oversized number, or a space inside the lists, \
which gets the no-spaces hint) reports the same. A token ending in a \
dangling separator (`x1,`, `x~`, `x*1,`, `x!2,`, `x*~`) reports \
mode 'incomplete' needing `pomodoro_close_task`, with the partial spec, the \
spans typed so far, and one `interactive_placeholder` span over the \
separator. A trailing `*`/`!` is a wildcard instead of dangling, so `x*` and `x!` are valid. Two empty wildcard groups conflict and require numbers on at least one group. A same-line chain splits into one `items[]` entry per operator, \
with child lines attaching to the line's `x` (the last `x` when it closes \
twice); a chain whose `x` is not last nests its item ranges, so they never \
partially overlap. Other `x` tokens (`xx`, `xa`, `x-ray`, a capitalized `X…`), other \
doubled and bare starts followed by prose (`== foo`, `= foo`), and mid-body \
`x` stay ordinary prose. The retired \
`=x…`, `=*…`, and `=!…` whole-item spellings report 'pomodoro_close' plus an \
`invalid_pomodoro_close` diagnostic that names the bare `x…` rewrite. On \
link items the `=x…` suffix (the `=` separates it from the block ID) spans the same four span kinds after the route \
and block-ID spans: `@r:id=x1!2` and `^r:id=x1` stay 'pomodoro_link' (or \
'pomodoro_task' with body text) and carry the spec, `^r:id=x1,` reports \
'incomplete' needing `pomodoro_close_task`, while `#name=x…`, \
`s:<N>`/`$<N>`/`%` conflicts, project-note `=x`, the retired `=*`/`=!` link aliases, and malformed lists \
report `invalid_pomodoro_close` on the conflicting component or the precise \
list range. A `@@` declaration never applies to close, `=`/`=<X>`, or \
`=name` items, and neither is ever rewritten. \
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
captured. A whole draft that is one `/` token (with no second `/`) reports \
'incomplete' needing `task_goto` with one `interactive_placeholder` span; \
`bob capture` refuses it. A `+query` token that is the item's whole remaining body after `&`/`*` \
modifiers leave (leading `+ &foo:bar` or trailing `&foo:bar +`) is an \
unresolved parent-task selector: it reports `incomplete` needing \
`task_parent` with a placeholder over the full token, keeps the modifiers, \
and capture refuses it until a task is selected. An exact lone `+` remains the existing \
Pomodoro adjustment in parsing. An `&note:block-id` modifier names a prerequisite task link \
(`&projects/foo:bar`; quote notes with spaces: `&\"Shopping List\":bar`): \
one or more modifiers lead or trail the parent line (interleaving with \
destination, schedule, priority, and clipboard markers in either order) or \
close an authored child line, and each adds a per-item `dependencies` entry \
(`raw`, decoded `note` and `block_id`, plus its sigil-inclusive `range`) \
with `dependency_sigil`, `dependency_note`, and `dependency_block_id` \
spans. Body text keeps its established mode and gains a `dependency_target` \
(`new_task` for a task this capture would create); a bare `&note:block-id` \
with an explicit `@note+task-id` (or the narrow bare `@note:id` / \
`^note:id` aliases, leading or trailing) reports mode 'task_dependency' \
with an `existing_task` target. A bare existing owner with `$<N>`, \
`s:<N>`, `%…`, or child bullets reports a `relationship_needs_text` \
diagnostic. An \
ownerless `&note:block-id` reports mode 'incomplete' needing \
`dependency_target`, and a partial `&`, `&query`, or `&note:` needs \
`task_dependency` with a placeholder span over the typed query. A mid-line \
`&` between prose words (`Research & Development`, `R&D`), an `&` inside \
`[[wikilinks]]` or `` `code` ``, and `\\&` (which leaves a visible `&...` in \
task text) stay literal. `bob capture` executes recognized modifiers \
through the staged batch writer (managed `DEPENDS ON` line plus derived \
field and status effects); a draft with prerequisites never captures \
without them. An additive `*note:block-id` modifier names an existing open task to block: \
the parsed task would become its prerequisite, reported separately in \
`blocked_tasks[]` with `blocked_task_sigil`, `blocked_task_note`, and \
`blocked_task_block_id` spans. It follows the same leading/trailing marker \
runs and ownership rules as `&`; `*` is also a partial picker need named \
`blocked_task`. A bare `*` opens that picker only as the last token after \
existing text, and line-initial `*` remains a Markdown bullet. Mid-line stars, \
`**bold**`, `*italic*`, and `5 * 3` stay literal. `bob capture` executes \
recognized `*` modifiers through the staged batch writer: the parsed task \
becomes a prerequisite of each named target (managed `DEPENDS ON` line plus \
derived field, status, consumption, and BLOCKS-mirror effects); a draft with \
blocked-task modifiers never captures without them. An additive `~note:block-id[|label]` \
modifier relates this task to another task (`~` links out, `~~` links back), \
optionally with a `|label` (quoted when it holds spaces: `|\"follow up\"`); \
each adds a per-item `relations[]` entry (`raw`, `direction`, decoded `note` \
and `block_id`, `label` with its `label_range`, plus its sigil-inclusive \
`range`) with `relation_sigil`, `relation_note`, `relation_block_id`, \
`relation_label_separator`, and `relation_label` spans, and any draft \
containing a `~` byte reports top-level `relation_grammar: 1` (drafts without \
`~` stay byte-identical). It follows the same leading/trailing marker runs \
and ownership rules as `&` (mid-line `~` stays body text; `~/path`, `~5m`, \
`~~old~~`, and `~~~x:y` stay literal with the documented guards; bare or \
partial `~` needs `relation_target`, and `~note:id|` needs `relation_label`); \
an ownerless `~note:block-id` reuses `dependency_target`, and `bob capture` \
executes recognized `~` / `~~` modifiers through the staged batch writer: it \
appends or relabels the source task's managed 🪢 LINKS line, mints any block \
IDs the edge needs, and stages the target's generated 🪢 LINKED FROM mirror; \
configured cancel and complete relation labels mark their `relation_label` \
spans with `cancels_source` and `completes_source`. Relations never block, \
consume, promote, or schedule anything. The retired \
'@route::...' spelling is a diagnostic directing users to '@route^...'; \
it is not an incomplete Pomodoro marker. A bare trailing '#' reports mode \
'pomodoro_note' with no route, section, or block ID and an empty 'needs' list; \
combining it with an '@route' marker, 's:<N>', or '$<N>' on the same item \
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
            "Examples:\n  bob capture-parse --format json -- 'Call bank @cash+'\n  bob capture-parse 'Call bank @Cash+'\n  bob capture-parse -f json -- 'jot idea @notes#Ideas'\n  bob capture-parse -f json -- 'Postgres 17 minimum @foo+bar#req'\n  bob capture-parse -f json -- '@cash+goog-exit'\n  bob capture-parse -f json -- '+5'\n  bob capture-parse -f json -- '-2'\n  bob capture-parse -f json -- '++3'\n  bob capture-parse -f json -- '--'\n  bob capture-parse -f json -- '+'\n  printf '++3\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  printf '+5\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  echo 'Do work @dev^focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123#' | bob capture-parse -f json\n  printf '@@foo\\nFirst task\\n\\nSecond task @bar\\n' | bob capture-parse -f json\n  printf 'Parent\\n- first child\\n\\nSecond @work\\n' | bob capture-parse\n  bob capture-parse -f json -- 'x'\n  bob capture-parse -f json -- 'x1,3!2'\n  bob capture-parse -f json -- 'x1~2'\n  bob capture-parse -f json -- 'x0!2'\n  printf 'x\\n- 1 wired the lexer\\n' | bob capture-parse -f json\n  printf 'x\\n- 1\\n' | bob capture-parse -f json\n  printf 'x =\\n- 1 wired it\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '='\n  bob capture-parse -f json -- '=3'\n  bob capture-parse -f json -- '=~2'\n  bob capture-parse -f json -- '=bugs'\n  bob capture-parse -f json -- '=3#bugs~1'\n  bob capture-parse -f json -- 'x =~2'\n  bob capture-parse -f json -- '@r:id=x'\n  bob capture-parse -f json -- '^r:id=x1'\n  printf 'x\\n\\n=\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '+2 x'\n  bob capture-parse -f json -- 'Buy Groceries! &foo:bar'\n  bob capture-parse -f json -- 'Ask Kelly *job:cleanup'\n  bob capture-parse -f json -- '&foo:bar @body+excercise'\n  bob capture-parse -f json -- '^r:id &foo:bar'\n  bob capture-parse -f json -- '&foo:bar +'\n  bob capture-parse -f json -- '&foo:bar'\n  bob capture-parse -f json -- 'Buy Groceries! &fo'\n  bob capture-parse -f json -- 'Fix trip ~work:foo'\n  bob capture-parse -f json -- 'Fix trip ~~work:foo|see'\n  bob capture-parse -f json -- '/fix'\n\nModes:\n  task, bullet, pomodoro_task, pomodoro_note, sub_bullet, task_toggle, project_note, pomodoro_project_note, pomodoro_adjust, pomodoro_shift, pomodoro_link, pomodoro_close, pomodoro_start, task_dependency, task_complete, ref, incomplete\n\nNeeds:\n  route, section, block_id, pomodoro_id, pomodoro_name, task, task_section, active_task, task_link, task_goto, task_parent, pomodoro_close_task, pomodoro_close_log_text, pomodoro_start_task, task_dependency, blocked_task, relation_target, relation_label, dependency_target, task_complete, task_type",
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
        .help(
            "Capture text; multiple args are joined with spaces; an unknown \
             `--flag` where TEXT would begin is rejected, later tokens stay literal",
        )
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
    /// Top-level `relation_grammar: 1` whenever the draft contains a `~`
    /// byte. Omitted otherwise, so drafts without relations stay
    /// byte-identical to today.
    #[serde(skip_serializing_if = "Option::is_none")]
    relation_grammar: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_parent: Option<RefParentParse>,
    /// Whether the standalone `%` marker (or compatible `@` alias)
    /// requested companion audio.
    /// Additive schema-version-1 field: omitted when false, so every
    /// older input keeps its exact JSON shape.
    #[serde(default, skip_serializing_if = "is_false")]
    ref_listen: bool,
    spans: Vec<CaptureParseSpan>,
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
    template: Option<TemplateParse>,
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
    /// Validated additive `=x` link close suffix: the typed `=x…` text as `raw`.
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_tasks: Vec<ParseDependency>,
    /// Complete `~` / `~~` modifiers on the first item, in source order.
    /// Omitted when the draft names none, so schema version 1 is unchanged
    /// for older inputs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relations: Vec<ParseRelation>,
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

/// One complete `~note:block-id[|label]` modifier with its sigil-inclusive
/// range and optional label range. `hidden` is true for the `||label`
/// spelling; it is omitted otherwise so drafts without `||` stay
/// byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseRelation {
    raw: String,
    direction: String,
    note: String,
    block_id: String,
    quoted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label_range: Option<SourceRange>,
    range: SourceRange,
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
    mode: String,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_parent: Option<RefParentParse>,
    #[serde(default, skip_serializing_if = "is_false")]
    ref_listen: bool,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_tasks: Vec<ParseDependency>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relations: Vec<ParseRelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency_target: Option<DependencyTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_complete: Option<ParseTaskComplete>,
    #[serde(skip_serializing_if = "Option::is_none")]
    template_row: Option<TemplateRowParse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CaptureParseSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: super::capture_language::SpanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) field: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) themes: Vec<ThemeRef>,
    /// True when a `relation_label` span's label is a configured cancel
    /// label (`relations.cancel_labels`). Omitted otherwise, so drafts
    /// without cancel labels stay byte-identical.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) cancels_source: bool,
    /// True when a `relation_label` span's label is configured to complete
    /// its source task (`relations.complete_labels`). Omitted otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) completes_source: bool,
}

fn is_theme_name_span(span: &CaptureParseSpan) -> bool {
    matches!(
        span.kind,
        capture_language::SpanKind::PomodoroName
            | capture_language::SpanKind::TaskTogglePomodoroName
            | capture_language::SpanKind::TaskType
    )
}

/// Decode one `task_type` span's marker token with the shared lexer: the
/// returned marker carries the decoded name for identity plus the authored
/// byte ranges for highlighting, or `None` for incomplete/invalid tokens.
fn task_type_marker(
    raw: &str,
    span_start: usize,
) -> Option<capture_language::TypeMarker> {
    scan_line(raw, span_start)
        .into_iter()
        .find_map(|occurrence| match occurrence {
            capture_language::TypeMarkerOccurrence::Complete(marker) => {
                Some(marker)
            }
            _ => None,
        })
}

fn theme_component_keys(
    result: &CaptureParseResult,
) -> std::collections::BTreeSet<String> {
    let mut keys = std::collections::BTreeSet::new();
    for span in &result.spans {
        if !is_theme_name_span(span) {
            continue;
        }
        let Some(raw) = result.input.get(span.start..span.end) else {
            continue;
        };
        if span.kind == capture_language::SpanKind::TaskType {
            let Some(marker) = task_type_marker(raw, span.start) else {
                continue;
            };
            let key = super::plan_budget::normalize_component(&marker.name);
            keys.insert(key.clone());
            keys.insert(key.replace('-', " "));
            continue;
        }
        let (name, _) = theme_name_content(raw, span.start);
        keys.extend(name.split('+').filter_map(|part| {
            let component = part.trim();
            (!component.is_empty())
                .then(|| super::plan_budget::normalize_component(component))
        }));
    }
    keys
}

fn resolve_task_type(
    catalog: &ThemeCatalog,
    name: &str,
    literal: bool,
) -> ResolvedTypeName {
    resolve_exact_type_name(
        catalog.configured_type_entries(),
        &|key| catalog.catalog_type_spelling(key),
        name,
        literal,
    )
    .unwrap_or_else(|| ResolvedTypeName {
        key: super::plan_budget::normalize_component(name),
        name: unmatched_type_display(name),
    })
}

fn attach_theme_refs(result: &mut CaptureParseResult, catalog: &ThemeCatalog) {
    for span in &mut result.spans {
        if !is_theme_name_span(span) {
            continue;
        }
        let Some(raw) = result.input.get(span.start..span.end) else {
            continue;
        };
        // A task-type span covers the whole marker token but carries one
        // ThemeRef over only the source name content. Names never split on
        // `+`: a task has exactly one type.
        if span.kind == capture_language::SpanKind::TaskType {
            let Some(marker) = task_type_marker(raw, span.start) else {
                continue;
            };
            let resolved =
                resolve_task_type(catalog, &marker.name, marker.literal);
            let mut reference =
                catalog.theme_ref(&resolved.key, &resolved.name);
            reference.start = Some(marker.name_span.start);
            reference.end = Some(marker.name_span.end);
            span.themes = vec![reference];
            continue;
        }
        let (name, base) = theme_name_content(raw, span.start);
        let mut refs = Vec::new();
        let mut offset = 0;
        for part in name.split('+') {
            let component = part.trim();
            let leading = part.len().saturating_sub(part.trim_start().len());
            if !component.is_empty() {
                let start = base + offset + leading;
                let end = start + component.len();
                let key = super::plan_budget::normalize_component(component);
                let mut reference = catalog.theme_ref(&key, component);
                reference.start = Some(start);
                reference.end = Some(end);
                refs.push(reference);
            }
            offset += part.len() + 1;
        }
        span.themes = refs;
    }
}

/// Exact-name spans include their quotes. Theme component offsets point at
/// the visible component text, excluding the literal quotes and `+` signs.
fn theme_name_content(raw: &str, span_start: usize) -> (&str, usize) {
    let (open, close) = if raw.starts_with('“') && raw.ends_with('”') {
        ("“".len(), "”".len())
    } else if raw.starts_with('"') && raw.ends_with('"') {
        (1, 1)
    } else {
        return (raw, span_start);
    };
    if raw.len() < open + close {
        return (raw, span_start);
    }
    (&raw[open..raw.len() - close], span_start + open)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TemplateParse {
    header: TemplateHeaderParse,
    range: SourceRange,
    line_start: usize,
    line_end: usize,
    field_count: usize,
    rows: Vec<TemplateRowParse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TemplateHeaderParse {
    range: SourceRange,
    line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TemplateRowParse {
    row: usize,
    line: usize,
    range: SourceRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<EditorMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    fields: Vec<TemplateFieldParse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TemplateFieldParse {
    field: usize,
    value: String,
    range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct SourceRange {
    start: usize,
    end: usize,
}

impl CaptureParseResult {
    fn new(input: String, options: &EditorParseOptions<'_>) -> Self {
        let analysis = capture_language::template::analyze(&input);
        if analysis.header.is_some() {
            return Self::new_template(input, options, analysis);
        }

        let parse = capture_language::parse_for_editor_with(&input, options);
        let mut spans = wire_spans(merge_spans(
            parse.spans,
            capture_links::wikilink_spans(&input),
        ));
        let close_policy = load_close_policy();
        stamp_cancel_spans(&mut spans, &input, &close_policy);
        stamp_complete_spans(&mut spans, &input, &close_policy);
        let items = parse_items(&parse.items);
        let ref_parent = ref_parent_for(
            parse.mode,
            parse.route.as_deref(),
            parse.items.first().map(|item| item.has_local_destination),
        );
        let ref_listen = parse.ref_listen;
        let sub_bullets = parse.sub_bullets;
        let sub_bullet_depths = sub_bullet_depths(&sub_bullets);
        let sub_bullet_task_ids = sub_bullet_task_ids(&sub_bullets);
        let mut diagnostics = Vec::new();
        if let Some(warning) = headerless_template_hint(&input, &parse.items) {
            diagnostics.push(warning);
        }
        diagnostics.extend(
            analysis
                .issues
                .iter()
                .filter(|issue| issue.code() == "template_header_misplaced")
                .map(template_issue_diagnostic),
        );
        diagnostics.extend(parse.diagnostics.into_iter().filter(
            |diagnostic| {
                !analysis.issues.iter().any(|issue| {
                    issue.code() == "template_header_misplaced"
                        && ranges_overlap(
                            diagnostic.range,
                            Some((issue.range().start, issue.range().end)),
                        )
                })
            },
        ));
        let relation_grammar = relation_grammar_for(&input);
        let relations = parse_relations(&parse.relations);
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
            relation_grammar,
            ref_parent,
            ref_listen,
            spans,
            diagnostics,
            sub_bullets: sub_bullet_bodies(&sub_bullets),
            sub_bullet_depths,
            sub_bullet_task_ids,
            items,
            template: None,
            global_destination: parse
                .global_destination
                .as_ref()
                .map(global_destination_parse),
            pomodoro_start: parse.pomodoro_start,
            pomodoro_adjust: parse.pomodoro_adjust,
            pomodoro_shift: parse.pomodoro_shift,
            pomodoro_close: parse.pomodoro_close,
            dependencies: parse_dependencies(&parse.dependencies),
            blocked_tasks: parse_dependencies(&parse.blocked_tasks),
            relations,
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

    fn new_template(
        input: String,
        options: &EditorParseOptions<'_>,
        analysis: capture_language::template::TemplateAnalysis,
    ) -> Self {
        let masked = capture_language::template::masked(&input);
        let parsed = capture_language::parse_for_editor_with(&masked, options);
        let placeholders = &analysis.placeholders;
        let template_needs =
            template_needs(&parsed.needs, &parsed.spans, placeholders);
        let body = parsed
            .items
            .first()
            .map(|item| rebuild_template_body(&input, &masked, item))
            .unwrap_or_default();

        let (header_range, header_line) = analysis
            .header
            .as_ref()
            .expect("template parse has a header");
        let (template_range, line_start, line_end) = analysis
            .block
            .as_ref()
            .map(|block| {
                (block.range.clone(), block.line_start, block.line_end)
            })
            .unwrap_or_else(|| {
                (header_range.clone(), *header_line, *header_line)
            });

        let expanded_rows = capture_language::template::expand_rows_for_editor(
            &input, &analysis,
        );
        let mut row_summaries = Vec::with_capacity(analysis.rows.len());
        let mut row_items = Vec::with_capacity(analysis.rows.len());
        let mut instance_diagnostics = Vec::new();
        let mut refusal_diagnostics = Vec::new();
        let mut incomplete_diagnostics = Vec::new();

        for (index, row) in analysis.rows.iter().enumerate() {
            let instance = expanded_rows.get(index).and_then(Option::as_ref);
            let row_parse = instance.map(|instance| {
                capture_language::parse_for_editor_with(&instance.text, options)
            });
            if let (Some(instance), Some(row_parse)) =
                (instance, row_parse.as_ref())
            {
                for diagnostic in &row_parse.diagnostics {
                    if diagnostic.code == "invalid_global_destination" {
                        continue;
                    }
                    let mut mapped = diagnostic.clone();
                    mapped.range = Some(map_instance_range(
                        diagnostic.range,
                        instance,
                        &row.fields,
                        &row.range,
                    ));
                    mapped.message = offset_diagnostic_lines(
                        &mapped.message,
                        instance.template_line_start.saturating_sub(1),
                    );
                    instance_diagnostics.push(mapped);
                }

                if let Some(diagnostic) = refusal_diagnostic(
                    &row_parse.spans,
                    row_parse.mode,
                    &row_parse.pomodoro_start,
                    &row_parse.pomodoro_close,
                    &row_parse.items,
                    (row.range.start, row.range.end),
                ) {
                    let mapped_range = map_instance_range(
                        diagnostic.range,
                        instance,
                        &row.fields,
                        &row.range,
                    );
                    refusal_diagnostics.push(Diagnostic {
                        range: Some(mapped_range),
                        ..diagnostic
                    });
                }

                if !row_parse.needs.is_empty()
                    || row_parse.mode == EditorMode::Incomplete
                {
                    incomplete_diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "template_row_incomplete",
                        message: format!(
                            "template row {} (line {}): complete the row's capture markers",
                            row.row, row.line
                        ),
                        range: Some((row.range.start, row.range.end)),
                    });
                }
            }

            let summary = TemplateRowParse {
                row: row.row,
                line: row.line,
                range: SourceRange {
                    start: row.range.start,
                    end: row.range.end,
                },
                mode: row_parse.as_ref().map(|parsed| parsed.mode),
                body: row_parse.as_ref().map(|parsed| parsed.body.clone()),
                fields: row
                    .fields
                    .iter()
                    .map(|field| TemplateFieldParse {
                        field: field.field,
                        value: field.value.clone(),
                        range: SourceRange {
                            start: field.range.start,
                            end: field.range.end,
                        },
                    })
                    .collect(),
            };

            if let Some(row_parse) = row_parse.as_ref() {
                let template_row = summary.clone();
                row_items.push(CaptureParseItem {
                    index: row.row + 1,
                    range: SourceRange {
                        start: row.range.start,
                        end: row.range.end,
                    },
                    line_start: row.line,
                    line_end: row.line,
                    body: row_parse.body.clone(),
                    mode: "template_row".to_string(),
                    route: row_parse.route.clone(),
                    section: row_parse.section.clone(),
                    block_id: row_parse.block_id.clone(),
                    needs: Vec::new(),
                    ref_parent: None,
                    ref_listen: row_parse.ref_listen,
                    sub_bullets: Vec::new(),
                    sub_bullet_depths: Vec::new(),
                    sub_bullet_task_ids: Vec::new(),
                    pomodoro_start: None,
                    pomodoro_adjust: None,
                    pomodoro_shift: None,
                    pomodoro_close: None,
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    task_complete: None,
                    template_row: Some(template_row),
                });
            } else {
                row_items.push(CaptureParseItem {
                    index: row.row + 1,
                    range: SourceRange {
                        start: row.range.start,
                        end: row.range.end,
                    },
                    line_start: row.line,
                    line_end: row.line,
                    body: String::new(),
                    mode: "template_row".to_string(),
                    route: None,
                    section: None,
                    block_id: None,
                    needs: Vec::new(),
                    ref_parent: None,
                    ref_listen: false,
                    sub_bullets: Vec::new(),
                    sub_bullet_depths: Vec::new(),
                    sub_bullet_task_ids: Vec::new(),
                    pomodoro_start: None,
                    pomodoro_adjust: None,
                    pomodoro_shift: None,
                    pomodoro_close: None,
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    task_complete: None,
                    template_row: Some(summary.clone()),
                });
            }
            row_summaries.push(summary);
        }

        // Wikilinks are intentionally scanned on the masked copy: row text
        // is data and must never be interpreted as editor syntax.
        let wikilinks = capture_links::wikilink_spans(&masked);
        let mut spans = template_spans(
            &parsed.spans,
            &wikilinks,
            &analysis,
            &row_summaries,
        );
        let close_policy = load_close_policy();
        stamp_cancel_spans(&mut spans, &masked, &close_policy);
        stamp_complete_spans(&mut spans, &masked, &close_policy);

        let mut diagnostics = analysis
            .issues
            .iter()
            .filter(|issue| issue.code() != "template_missing_rows")
            .map(template_issue_diagnostic)
            .collect::<Vec<_>>();

        diagnostics.extend(parsed.diagnostics.iter().filter_map(
            |diagnostic| {
                if diagnostic.code == "invalid_global_destination"
                    || diagnostic.range.is_some_and(|range| {
                        placeholders.iter().any(|placeholder| {
                            ranges_overlap(
                                Some(range),
                                Some((
                                    placeholder.range.start,
                                    placeholder.range.end,
                                )),
                            )
                        })
                    })
                {
                    None
                } else {
                    Some(diagnostic.clone())
                }
            },
        ));
        diagnostics.extend(instance_diagnostics);

        // Instance parses are the source of truth for clipboard: the masked
        // copy (`xxx %`) reads as a clipboard marker while a `URL %` row
        // expands to a Listen reference. Only the session check runs here.
        if let Some(diagnostic) = refusal_diagnostic(
            &parsed.spans,
            parsed.mode,
            &parsed.pomodoro_start,
            &parsed.pomodoro_close,
            &parsed.items,
            (template_range.start, template_range.end),
        ) && diagnostic.code != "template_clipboard"
        {
            refusal_diagnostics.push(diagnostic);
        }
        diagnostics.extend(refusal_diagnostics);
        diagnostics.extend(incomplete_diagnostics);
        diagnostics.extend(
            analysis
                .issues
                .iter()
                .filter(|issue| issue.code() == "template_missing_rows")
                .map(template_issue_diagnostic),
        );
        deduplicate_diagnostics(&mut diagnostics);

        let mut template_item = parsed
            .items
            .first()
            .map(wire_item)
            .unwrap_or_else(empty_wire_item);
        template_item.index = 1;
        template_item.range = SourceRange {
            start: template_range.start,
            end: template_range.end,
        };
        template_item.line_start = line_start;
        template_item.line_end = line_end;
        template_item.body = body.clone();
        template_item.needs = template_needs.clone();
        template_item.template_row = None;
        let mut items = Vec::with_capacity(row_items.len() + 1);
        items.push(template_item);
        items.extend(row_items);

        let header = TemplateHeaderParse {
            range: SourceRange {
                start: header_range.start,
                end: header_range.end,
            },
            line: *header_line,
        };
        let template = TemplateParse {
            header,
            range: SourceRange {
                start: template_range.start,
                end: template_range.end,
            },
            line_start,
            line_end,
            field_count: analysis.field_count,
            rows: row_summaries,
        };

        let mut needs = template_needs;
        if analysis.block.is_none() || analysis.rows.is_empty() {
            needs.push(Need::TemplateRow);
        }
        let ref_parent = ref_parent_for(
            parsed.mode,
            parsed.route.as_deref(),
            parsed.items.first().map(|item| item.has_local_destination),
        );
        let sub_bullets = parsed.sub_bullets;
        let sub_bullet_depths = sub_bullet_depths(&sub_bullets);
        let sub_bullet_task_ids = sub_bullet_task_ids(&sub_bullets);

        let relation_grammar = relation_grammar_for(&input);
        let relations = parse_relations(&parsed.relations);
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            input,
            body,
            mode: parsed.mode,
            route: parsed.route,
            section: parsed.section,
            block_id: parsed.block_id,
            needs,
            relation_grammar,
            ref_parent,
            ref_listen: parsed.ref_listen,
            spans,
            diagnostics,
            sub_bullets: sub_bullet_bodies(&sub_bullets),
            sub_bullet_depths,
            sub_bullet_task_ids,
            items,
            template: Some(template),
            global_destination: None,
            pomodoro_start: parsed.pomodoro_start,
            pomodoro_adjust: parsed.pomodoro_adjust,
            pomodoro_shift: parsed.pomodoro_shift,
            pomodoro_close: parsed.pomodoro_close,
            dependencies: parse_dependencies(&parsed.dependencies),
            blocked_tasks: parse_dependencies(&parsed.blocked_tasks),
            relations,
            dependency_target: parsed.dependency_target,
            task_complete: parsed.task_complete.as_ref().map(|entry| {
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

fn parse_relations(entries: &[RelationEntry]) -> Vec<ParseRelation> {
    entries
        .iter()
        .map(|entry| {
            let direction = match entry.direction {
                RelationDirection::To => "to".to_string(),
                RelationDirection::From => "from".to_string(),
            };
            let label_range = match (entry.label_start, entry.label_end) {
                (Some(start), Some(end)) => Some(SourceRange { start, end }),
                _ => None,
            };
            ParseRelation {
                raw: entry.raw.clone(),
                direction,
                note: entry.note.clone(),
                block_id: entry.block_id.clone(),
                quoted: entry.quoted,
                hidden: entry.hidden,
                label: entry.label.clone(),
                label_range,
                range: SourceRange {
                    start: entry.start,
                    end: entry.end,
                },
            }
        })
        .collect()
}

fn relation_grammar_for(input: &str) -> Option<u8> {
    input.contains('~').then_some(1)
}

/// Best-effort cancel-label policy for span flags: a broken config
/// means fewer flags, never a failed parse.
fn load_close_policy() -> crate::native::config::RelationsConfig {
    match crate::native::config::load_relations_config(
        &crate::native::config::config_path(),
    ) {
        Ok(policy) => policy,
        Err(error) => crate::native::config::RelationsConfig {
            linked_from_lines: true,
            linked_from_mentions: false,
            cancel_labels: Vec::new(),
            complete_labels: Vec::new(),
            warnings: vec![format!(
                "invalid relations config: {error}; closing nothing"
            )],
        },
    }
}

/// Mark `relation_label` spans whose label is configured to complete the
/// source. Omitted (false) everywhere else.
fn stamp_complete_spans(
    spans: &mut [CaptureParseSpan],
    input: &str,
    policy: &crate::native::config::RelationsConfig,
) {
    if policy.complete_labels.is_empty() {
        return;
    }
    for span in spans.iter_mut() {
        if span.kind != crate::native::capture_language::SpanKind::RelationLabel
        {
            continue;
        }
        let Some(text) = input.get(span.start..span.end) else {
            continue;
        };
        if policy.is_complete_label(
            &crate::native::relation_labels::relation_label_key(text),
        ) {
            span.completes_source = true;
        }
    }
}

/// Mark `relation_label` spans whose label is a configured cancel
/// label. Omitted (false) everywhere else, so output without cancel
/// labels stays byte-identical.
fn stamp_cancel_spans(
    spans: &mut [CaptureParseSpan],
    input: &str,
    policy: &crate::native::config::RelationsConfig,
) {
    if policy.cancel_labels.is_empty() {
        return;
    }
    for span in spans.iter_mut() {
        if span.kind != crate::native::capture_language::SpanKind::RelationLabel
        {
            continue;
        }
        let Some(text) = input.get(span.start..span.end) else {
            continue;
        };
        if policy.is_cancel_label(
            &crate::native::relation_labels::relation_label_key(text),
        ) {
            span.cancels_source = true;
        }
    }
}

fn wire_spans(spans: Vec<Span>) -> Vec<CaptureParseSpan> {
    spans
        .into_iter()
        .map(|span| CaptureParseSpan {
            start: span.start,
            end: span.end,
            kind: span.kind,
            field: None,
            themes: Vec::new(),
            cancels_source: false,
            completes_source: false,
        })
        .collect()
}

fn template_issue_diagnostic(
    issue: &capture_language::template::TemplateIssue,
) -> Diagnostic {
    let range = issue.range();
    Diagnostic {
        severity: Severity::Error,
        code: issue.code(),
        message: issue.message(),
        range: Some((range.start, range.end)),
    }
}

fn ranges_overlap(
    left: Option<(usize, usize)>,
    right: Option<(usize, usize)>,
) -> bool {
    let (Some((left_start, left_end)), Some((right_start, right_end))) =
        (left, right)
    else {
        return false;
    };
    left_start < right_end && right_start < left_end
}

fn template_needs(
    needs: &[Need],
    spans: &[Span],
    placeholders: &[capture_language::template::TemplatePlaceholder],
) -> Vec<Need> {
    if spans.iter().any(|span| {
        span.kind == capture_language::SpanKind::InteractivePlaceholder
            && placeholders.iter().any(|placeholder| {
                ranges_overlap(
                    Some((span.start, span.end)),
                    Some((placeholder.range.start, placeholder.range.end)),
                )
            })
    }) {
        return Vec::new();
    }
    needs
        .iter()
        .copied()
        .filter(|need| !matches!(need, Need::BlockId | Need::PomodoroId))
        .collect()
}

fn template_spans(
    capture_spans: &[Span],
    wikilink_spans: &[Span],
    analysis: &capture_language::template::TemplateAnalysis,
    rows: &[TemplateRowParse],
) -> Vec<CaptureParseSpan> {
    use capture_language::SpanKind;

    let merged = merge_spans(capture_spans.to_vec(), wikilink_spans.to_vec());
    let mut result = Vec::new();
    for span in merged {
        if span.start >= span.end {
            continue;
        }
        let overlaps_placeholder = analysis.placeholders.iter().any(|item| {
            ranges_overlap(
                Some((span.start, span.end)),
                Some((item.range.start, item.range.end)),
            )
        });
        if span.kind == SpanKind::InteractivePlaceholder && overlaps_placeholder
        {
            continue;
        }
        let mut cursor = span.start;
        for placeholder in &analysis.placeholders {
            if placeholder.range.end <= cursor
                || placeholder.range.start >= span.end
            {
                continue;
            }
            if cursor < placeholder.range.start {
                result.push(CaptureParseSpan {
                    start: cursor,
                    end: placeholder.range.start,
                    kind: span.kind,
                    field: None,
                    themes: Vec::new(),
                    cancels_source: false,
                    completes_source: false,
                });
            }
            cursor = cursor.max(placeholder.range.end);
        }
        if cursor < span.end {
            result.push(CaptureParseSpan {
                start: cursor,
                end: span.end,
                kind: span.kind,
                field: None,
                themes: Vec::new(),
                cancels_source: false,
                completes_source: false,
            });
        }
    }

    if let Some((range, _)) = analysis.header.as_ref() {
        result.push(CaptureParseSpan {
            start: range.start,
            end: range.end,
            kind: SpanKind::TemplateHeader,
            field: None,
            themes: Vec::new(),
            cancels_source: false,
            completes_source: false,
        });
    }
    result.extend(analysis.placeholders.iter().map(|placeholder| {
        CaptureParseSpan {
            start: placeholder.range.start,
            end: placeholder.range.end,
            kind: SpanKind::TemplatePlaceholder,
            field: Some(placeholder.field),
            themes: Vec::new(),
            cancels_source: false,
            completes_source: false,
        }
    }));
    result.extend(rows.iter().flat_map(|row| {
        row.fields.iter().map(|field| CaptureParseSpan {
            start: field.range.start,
            end: field.range.end,
            kind: SpanKind::TemplateField,
            field: Some(field.field),
            themes: Vec::new(),
            cancels_source: false,
            completes_source: false,
        })
    }));
    result.sort_by_key(|span| (span.start, span.end));
    result
}

fn rebuild_template_body(
    raw: &str,
    masked: &str,
    item: &EditorItemParse,
) -> String {
    let lines = capture_language::split_physical_lines(raw);
    let Some(line) = lines.get(item.line_start.saturating_sub(1)) else {
        return item.body.clone();
    };
    let masked_line = &masked[line.start..line.end];
    let mut kept = Vec::new();
    for token in capture_language::tokenize_with_spans(masked_line) {
        let start = line.start + token.start;
        let end = line.start + token.end;
        let capture_span = item.spans.iter().any(|span| {
            span.kind != capture_language::SpanKind::WikilinkDelimiter
                && span.kind != capture_language::SpanKind::WikilinkTarget
                && span.kind != capture_language::SpanKind::WikilinkHeading
                && span.kind != capture_language::SpanKind::WikilinkBlockId
                && span.kind != capture_language::SpanKind::WikilinkAlias
                && ranges_overlap(
                    Some((start, end)),
                    Some((span.start, span.end)),
                )
        });
        let diagnostic_span = item.diagnostics.iter().any(|diagnostic| {
            ranges_overlap(diagnostic.range, Some((start, end)))
        });
        if !capture_span && !diagnostic_span {
            kept.push(raw[start..end].to_string());
        }
    }
    if kept.is_empty() {
        String::new()
    } else {
        kept.join(" ")
    }
}

fn headerless_template_hint(
    raw: &str,
    items: &[EditorItemParse],
) -> Option<Diagnostic> {
    let first = items.first()?;
    if items.len() < 2 || !contains_braced_decimal(&raw[first.start..first.end])
    {
        return None;
    }
    Some(Diagnostic {
        severity: Severity::Warning,
        code: "template_header_missing",
        message: "add `@@template:` as the first line to use this item as a template for the lines below".to_string(),
        range: Some((first.start, first.end)),
    })
}

fn contains_braced_decimal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start && bytes.get(end) == Some(&b'}') {
            return true;
        }
        index += 1;
    }
    false
}

fn map_instance_range(
    range: Option<(usize, usize)>,
    instance: &capture_language::template::TemplateInstance,
    fields: &[capture_language::template::TemplateField],
    row_range: &std::ops::Range<usize>,
) -> (usize, usize) {
    let Some((start, end)) = range else {
        return (row_range.start, row_range.end);
    };
    let touched = instance
        .source_map
        .iter()
        .filter(|segment| {
            ranges_overlap(
                Some((start, end)),
                Some((segment.expanded.start, segment.expanded.end)),
            )
        })
        .collect::<Vec<_>>();
    let field_ids = touched
        .iter()
        .filter_map(|segment| segment.field)
        .collect::<std::collections::BTreeSet<_>>();
    if field_ids.len() == 1 {
        if let Some(field) = fields
            .iter()
            .find(|field| Some(field.field) == field_ids.first().copied())
        {
            return (field.range.start, field.range.end);
        }
    } else if field_ids.len() > 1 {
        return (row_range.start, row_range.end);
    }
    let literal_sources = touched
        .iter()
        .filter(|segment| segment.field.is_none())
        .map(|segment| {
            let overlap_start = start.max(segment.expanded.start);
            let overlap_end = end.min(segment.expanded.end);
            if segment.source.len() == segment.expanded.len() {
                let offset_start = overlap_start - segment.expanded.start;
                let offset_end = overlap_end - segment.expanded.start;
                (
                    segment.source.start + offset_start,
                    segment.source.start + offset_end,
                )
            } else {
                (segment.source.start, segment.source.end)
            }
        })
        .collect::<Vec<_>>();
    if let (Some(first), Some(last)) = (
        literal_sources.iter().map(|range| range.0).min(),
        literal_sources.iter().map(|range| range.1).max(),
    ) {
        return (first, last);
    }
    (row_range.start, row_range.end)
}

fn offset_diagnostic_lines(message: &str, offset: usize) -> String {
    if offset == 0 {
        return message.to_string();
    }
    let mut output = String::with_capacity(message.len());
    let mut cursor = 0;
    while let Some(relative) = message[cursor..].find("line ") {
        let start = cursor + relative;
        let digits_start = start + "line ".len();
        let digits_end = message[digits_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count()
            + digits_start;
        if digits_end == digits_start {
            output.push_str(&message[cursor..digits_start]);
            cursor = digits_start;
            continue;
        }
        output.push_str(&message[cursor..digits_start]);
        if let Ok(line) = message[digits_start..digits_end].parse::<usize>() {
            output.push_str(&(line.saturating_add(offset)).to_string());
        } else {
            output.push_str(&message[digits_start..digits_end]);
        }
        cursor = digits_end;
    }
    output.push_str(&message[cursor..]);
    output
}

fn refusal_diagnostic(
    spans: &[Span],
    mode: EditorMode,
    pomodoro_start: &Option<PomodoroStartSpec>,
    pomodoro_close: &Option<PomodoroCloseSpec>,
    items: &[EditorItemParse],
    fallback_range: (usize, usize),
) -> Option<Diagnostic> {
    use capture_language::SpanKind;

    if let Some(span) =
        spans.iter().find(|span| span.kind == SpanKind::Clipboard)
    {
        return Some(Diagnostic {
            severity: Severity::Error,
            code: "template_clipboard",
            message: "a template can't capture the clipboard: `%` would repeat once per row".to_string(),
            range: Some((span.start, span.end)),
        });
    }
    let is_session = |mode: EditorMode,
                      start: &Option<PomodoroStartSpec>,
                      close: &Option<PomodoroCloseSpec>| {
        matches!(
            mode,
            EditorMode::PomodoroAdjust
                | EditorMode::PomodoroShift
                | EditorMode::PomodoroClose
                | EditorMode::PomodoroStart
        ) || matches!(mode, EditorMode::PomodoroTask | EditorMode::PomodoroLink)
            && (start.is_some() || close.is_some())
    };
    let session = is_session(mode, pomodoro_start, pomodoro_close)
        || items.iter().any(|item| {
            is_session(item.mode, &item.pomodoro_start, &item.pomodoro_close)
        });
    if !session {
        return None;
    }
    let range = spans
        .iter()
        .find(|span| {
            matches!(
                span.kind,
                SpanKind::PomodoroAdjust
                    | SpanKind::PomodoroShift
                    | SpanKind::PomodoroClose
                    | SpanKind::PomodoroStart
            )
        })
        .map(|span| (span.start, span.end))
        .unwrap_or(fallback_range);
    Some(Diagnostic {
        severity: Severity::Error,
        code: "template_session_operator",
        message: "a template can't drive the Pomodoro session; capture `=`, `x`, `+N`, and `++N` on their own".to_string(),
        range: Some(range),
    })
}

fn deduplicate_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = std::collections::HashSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code,
            diagnostic.range,
            diagnostic.message.clone(),
        ))
    });
}

fn empty_wire_item() -> CaptureParseItem {
    CaptureParseItem {
        index: 1,
        range: SourceRange { start: 0, end: 0 },
        line_start: 1,
        line_end: 1,
        body: String::new(),
        mode: EditorMode::Task.label().to_string(),
        route: None,
        section: None,
        block_id: None,
        needs: Vec::new(),
        ref_parent: None,
        ref_listen: false,
        sub_bullets: Vec::new(),
        sub_bullet_depths: Vec::new(),
        sub_bullet_task_ids: Vec::new(),
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_close: None,
        dependencies: Vec::new(),
        blocked_tasks: Vec::new(),
        relations: Vec::new(),
        dependency_target: None,
        task_complete: None,
        template_row: None,
    }
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
    items.iter().map(wire_item).collect()
}

fn wire_item(item: &EditorItemParse) -> CaptureParseItem {
    CaptureParseItem {
        index: item.index + 1,
        range: SourceRange {
            start: item.start,
            end: item.end,
        },
        line_start: item.line_start,
        line_end: item.line_end,
        body: item.body.clone(),
        mode: item.mode.label().to_string(),
        route: item.route.clone(),
        section: item.section.clone(),
        block_id: item.block_id.clone(),
        needs: item.needs.clone(),
        ref_parent: ref_parent_for(
            item.mode,
            item.route.as_deref(),
            Some(item.has_local_destination),
        ),
        ref_listen: item.ref_listen,
        sub_bullets: sub_bullet_bodies(&item.sub_bullets),
        sub_bullet_depths: sub_bullet_depths(&item.sub_bullets),
        sub_bullet_task_ids: sub_bullet_task_ids(&item.sub_bullets),
        pomodoro_start: item.pomodoro_start.clone(),
        pomodoro_adjust: item.pomodoro_adjust.clone(),
        pomodoro_shift: item.pomodoro_shift.clone(),
        pomodoro_close: item.pomodoro_close.clone(),
        dependencies: parse_dependencies(&item.dependencies),
        blocked_tasks: parse_dependencies(&item.blocked_tasks),
        relations: parse_relations(&item.relations),
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
        template_row: None,
    }
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

fn is_false(value: &bool) -> bool {
    !*value
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
    if let Some(template) = result.template.as_ref() {
        print_field(
            styler,
            "template",
            &format!(
                "{} fields · {} rows",
                template.field_count,
                template.rows.len()
            ),
        );
    }
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
    if !result.blocked_tasks.is_empty() {
        let blocked_tasks = result
            .blocked_tasks
            .iter()
            .map(|entry| entry.raw.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        print_field(styler, "blocked tasks", &blocked_tasks);
    }
    if !result.relations.is_empty() {
        let relations = result
            .relations
            .iter()
            .map(|entry| entry.raw.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        print_field(styler, "relations", &relations);
    }
    if let Some(grammar) = result.relation_grammar {
        print_field(styler, "relation grammar", &grammar.to_string());
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
/// typed token (`=` plus `<X>`, plus `#name` for a named start plus
/// `~<K>` for a drop list) plus its resolved 5-minute duration and offset
/// units.
fn format_pomodoro_start(
    start: &PomodoroStartSpec,
    section: Option<&str>,
) -> String {
    let sigil = "=";
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

/// Render a validated `x[<N>][*<P>][!<M>][~<K>]` close for human output: the
/// typed token exactly as entered plus the outcome summary. A plain close
/// prints alone; a typed
/// `<N>` list without a wildcard ends with `defer the rest`, and `x0` reads
/// `in progress none`. Typed Work Log entries render as
/// `log 2 "wired the lexer"`, one per entry in typed order, each followed
/// by `(+N detail)`/`(+N details)` when it carries details. Entries whose
/// index resolves at execution render as `log "text"`, with no number.
fn format_pomodoro_close(close: &PomodoroCloseSpec) -> String {
    let base = close.raw.clone();
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
            // `x0*2` records work via parking: do not claim
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

    fn task_type_catalog() -> (tempfile::TempDir, ThemeCatalog) {
        let vault = tempfile::tempdir().expect("scratch vault");
        std::fs::create_dir_all(vault.path().join("2026")).unwrap();
        std::fs::write(
            vault.path().join("2026/20261023.md"),
            "## Pomodoros\n- [x] (0800-0830 30m) — FIX\n- [x] (0900-0930 30m) — DEEP WORK\n",
        )
        .unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let keys = std::collections::BTreeSet::from([
            "fix".to_string(),
            "deep work".to_string(),
        ]);
        let catalog =
            ThemeCatalog::load_for_theme_keys(vault.path(), today, &keys)
                .expect("theme catalog");
        (vault, catalog)
    }

    fn task_type_themes(
        input: &str,
        catalog: &ThemeCatalog,
    ) -> serde_json::Value {
        let options = EditorParseOptions::routing_off();
        let mut result = CaptureParseResult::new(input.to_string(), &options);
        attach_theme_refs(&mut result, catalog);
        let value = serde_json::to_value(&result).unwrap();
        value["spans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|span| span["kind"] == "task_type")
            .expect("task_type span")["themes"]
            .clone()
    }

    #[test]
    fn task_type_spans_carry_one_ref_over_the_name_content() {
        let (_vault, catalog) = task_type_catalog();
        // A catalog slug match resolves through the hyphen fallback.
        let themes = task_type_themes("Fix it t:deep-work", &catalog);
        assert_eq!(themes.as_array().unwrap().len(), 1);
        assert_eq!(themes[0]["key"], "deep work");
        assert_eq!(themes[0]["name"], "DEEP WORK");
        assert!(themes[0].get("known").is_none());
        assert_eq!(
            &"Fix it t:deep-work"[themes[0]["start"].as_u64().unwrap() as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "deep-work"
        );
        // A literal resolves by key equality with quotes excluded.
        let themes = task_type_themes("Fix it t:\"DEEP WORK\"", &catalog);
        assert_eq!(themes[0]["key"], "deep work");
        assert_eq!(
            &"Fix it t:\"DEEP WORK\""[themes[0]["start"].as_u64().unwrap()
                as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "DEEP WORK"
        );
        // Curly quotes keep char-boundary name ranges.
        let themes = task_type_themes("Fix it t:“FIX”", &catalog);
        assert_eq!(themes[0]["key"], "fix");
        assert_eq!(
            &"Fix it t:“FIX”"[themes[0]["start"].as_u64().unwrap() as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "FIX"
        );
        // An unknown name carries no fabricated resolution.
        let themes = task_type_themes("Fix it t:nope", &catalog);
        assert_eq!(themes.as_array().unwrap().len(), 1);
        assert_eq!(themes[0]["key"], "nope");
        assert_eq!(themes[0]["name"], "NOPE");
        assert_eq!(themes[0]["known"], false);
        // Quoted composites are invalid markers and never fabricate a ref.
        for raw in ["Fix it t:\"a+b\"", "Fix it t:“a+b”"] {
            let mut result = CaptureParseResult::new(
                raw.to_string(),
                &EditorParseOptions::routing_off(),
            );
            attach_theme_refs(&mut result, &catalog);
            let marker = result
                .spans
                .iter()
                .find(|span| span.kind == capture_language::SpanKind::TaskType)
                .expect("invalid task_type span");
            assert!(marker.themes.is_empty(), "{raw:?}: {marker:?}");
            assert!(
                result.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "invalid_type_marker"
                }),
                "{raw:?}: {:?}",
                result.diagnostics
            );
        }
    }

    #[test]
    fn incomplete_type_markers_attach_no_theme_ref() {
        let (_vault, catalog) = task_type_catalog();
        let options = EditorParseOptions::routing_off();
        let mut result =
            CaptureParseResult::new("Fix it t:".to_string(), &options);
        attach_theme_refs(&mut result, &catalog);
        let value = serde_json::to_value(&result).unwrap();
        let span = value["spans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|span| span["kind"] == "task_type")
            .expect("task_type span");
        assert!(span.get("themes").is_none());
    }

    #[test]
    fn theme_name_spans_report_components_with_source_offsets() {
        let vault = tempfile::tempdir().expect("scratch vault");
        std::fs::create_dir_all(vault.path().join("2026")).unwrap();
        std::fs::write(
            vault.path().join("2026/20261023.md"),
            "## Pomodoros\n- [x] (0800-0830 30m) — BOB + DECKS\n",
        )
        .unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let keys = std::collections::BTreeSet::from([
            "new".to_string(),
            "bob".to_string(),
        ]);
        let catalog =
            ThemeCatalog::load_for_theme_keys(vault.path(), today, &keys)
                .expect("theme catalog");
        let input = r###"=#"NEW + BOB""###;
        let options = EditorParseOptions::routing_off();
        let mut result = CaptureParseResult::new(input.to_string(), &options);
        attach_theme_refs(&mut result, &catalog);
        let value = serde_json::to_value(&result).unwrap();
        let span = value["spans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|span| span["kind"] == "pomodoro_name")
            .expect("name span");
        let themes = span["themes"].as_array().expect("theme refs");
        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0]["name"], "NEW");
        assert_eq!(themes[0]["known"], false);
        assert_eq!(themes[1]["name"], "BOB");
        // Known refs omit `known`; only unknown refs serialize `known: false`.
        assert!(themes[1].get("known").is_none());
        assert_eq!(
            &input[themes[0]["start"].as_u64().unwrap() as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "NEW"
        );
        assert_eq!(
            &input[themes[1]["start"].as_u64().unwrap() as usize
                ..themes[1]["end"].as_u64().unwrap() as usize],
            "BOB"
        );
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

        let value = json("body $2 s:1 % @groceries");
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
        let value = json("Do work @sase:outline=3abc");
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
        let value = json("x");
        assert_eq!(value["mode"], "pomodoro_close");
        assert_eq!(value["body"], "x");
        assert_eq!(value["pomodoro_close"]["raw"], "x");
        assert_eq!(value["spans"][0]["kind"], "pomodoro_close");
        assert_eq!(value["spans"][0]["start"], 0);
        assert_eq!(value["spans"][0]["end"], 1);
        assert!(value["diagnostics"].as_array().expect("diags").is_empty());

        // A capitalized `X` is ordinary prose.
        let upper = json("X");
        assert_eq!(upper["mode"], "task");
        assert!(upper.get("pomodoro_close").is_none());

        // The retired `=x` close is a claimed teaching error.
        let retired = json("=x");
        assert_eq!(retired["mode"], "pomodoro_close");
        assert!(retired.get("pomodoro_close").is_none());
        assert_eq!(retired["diagnostics"][0]["code"], "invalid_pomodoro_close");
        assert_eq!(
            retired["diagnostics"][0]["range"],
            serde_json::json!([0, 2])
        );
        assert_eq!(
            retired["diagnostics"][0]["message"],
            "`=x` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x`"
        );

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

        let shape = json("x more");
        assert_eq!(shape["mode"], "pomodoro_close");
        assert_eq!(
            shape["pomodoro_close"]["log"],
            serde_json::json!([{ "index": 1, "text": "more" }])
        );
        assert_eq!(shape["body"], "x");
        assert!(shape["diagnostics"].as_array().expect("diags").is_empty());

        let named = json("@r:id#n=x");
        assert_eq!(named["diagnostics"][0]["code"], "invalid_pomodoro_close");

        let scheduled = json("Text @r:id=x s:2");
        assert_eq!(
            scheduled["diagnostics"][0]["code"],
            "invalid_pomodoro_close"
        );

        let raw = "Plan x";
        let prose = json(raw);
        assert_eq!(prose["mode"], "task", "{raw}");
        assert!(prose.get("pomodoro_close").is_none(), "{raw}");
        assert!(prose.get("pomodoro_start").is_none(), "{raw}");

        let raw = "+5\n\nx\n\nCall bank @Cash+";
        let mixed = json(raw);
        assert_eq!(mixed["items"].as_array().expect("items").len(), 3);
        assert_eq!(mixed["items"][0]["mode"], "pomodoro_adjust");
        assert_eq!(mixed["items"][1]["mode"], "pomodoro_close");
        assert_eq!(mixed["items"][1]["pomodoro_close"]["raw"], "x");
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

        let close_hash = json("x#bugs");
        assert_eq!(close_hash["mode"], "pomodoro_close");
        assert_eq!(
            close_hash["diagnostics"][0]["code"],
            "invalid_pomodoro_close"
        );
        assert_eq!(
            close_hash["diagnostics"][0]["range"],
            serde_json::json!([1, 6])
        );
        assert_eq!(
            close_hash["spans"],
            serde_json::json!([
                { "start": 0, "end": 1, "kind": "pomodoro_close" },
            ])
        );

        let chain = json("x =#bugs");
        assert_eq!(chain["mode"], "pomodoro_close");
        assert_eq!(chain["items"].as_array().expect("items").len(), 2);
        assert_eq!(chain["items"][0]["mode"], "pomodoro_close");
        assert_eq!(chain["items"][1]["mode"], "pomodoro_start");
        assert_eq!(chain["items"][1]["section"], "bugs");
        assert_eq!(chain["items"][1]["body"], "=#bugs");
        assert_eq!(
            chain["items"][1]["range"],
            serde_json::json!({ "start": 2, "end": 8 })
        );

        let bare = json("=bugs");
        assert_eq!(bare["mode"], "pomodoro_start");
        assert_eq!(bare["section"], "bugs");
        assert_eq!(
            bare["spans"],
            serde_json::json!([
                { "start": 0, "end": 1, "kind": "pomodoro_start" },
                { "start": 1, "end": 5, "kind": "pomodoro_name" },
            ])
        );
        assert_eq!(bare["pomodoro_start"], value["pomodoro_start"]);
        let bare_chain = json("x =bugs");
        assert_eq!(bare_chain["items"][1]["section"], "bugs");
        assert_eq!(bare_chain["items"][1]["body"], "=bugs");
        assert_eq!(
            bare_chain["items"][1]["pomodoro_start"],
            chain["items"][1]["pomodoro_start"]
        );
    }

    #[test]
    fn json_reports_retired_doubled_starts_with_token_ranges() {
        for raw in ["==", "==3#bugs", "==#", "==~", "==#bugs=3"] {
            let value = json(raw);
            assert_eq!(value["mode"], "pomodoro_start", "{raw}: {value}");
            assert_eq!(value["body"], raw, "{raw}: {value}");
            assert!(value.get("pomodoro_start").is_none(), "{raw}: {value}");
            assert_eq!(
                value["diagnostics"][0]["code"], "invalid_pomodoro_start",
                "{raw}: {value}"
            );
            assert_eq!(
                value["diagnostics"][0]["range"],
                serde_json::json!([0, raw.len()]),
                "{raw}: {value}"
            );
            assert_eq!(
                value["spans"],
                serde_json::json!([
                    { "start": 0, "end": raw.len(), "kind": "pomodoro_start" }
                ]),
                "{raw}: {value}"
            );
            let rewrite = raw.strip_prefix('=').expect("doubled token");
            assert_eq!(
                value["diagnostics"][0]["message"],
                format!("`{raw}` is retired: `=` restarts or swaps the running Pomodoro by itself; write `{rewrite}`"),
                "{raw}: {value}"
            );
        }

        for raw in ["== foo", "==important== thing"] {
            let value = json(raw);
            assert_eq!(value["mode"], "task", "{raw}: {value}");
            assert_eq!(value["diagnostics"], serde_json::json!([]));
        }
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
            raw: "x1,2".to_string(),
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
            "x1,2 (in progress 1, 2 · log 1 \"wired the lexer\" (+1 detail), 2 \"sketched the parser\" (+2 details), 1 \"opened the PR\" · defer the rest)"
        );
        let plain = PomodoroCloseSpec::plain("x".to_string());
        assert_eq!(format_pomodoro_close(&plain), "x");
    }

    #[test]
    fn format_pomodoro_close_describes_wildcard_scope() {
        let bare_park = PomodoroCloseSpec {
            raw: "x*".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: true,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(format_pomodoro_close(&bare_park), "x* (parked all)");

        let park_except_complete = PomodoroCloseSpec {
            raw: "x*!2".to_string(),
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
            "x*!2 (parked the remaining links · complete 2)"
        );

        let progress_exception = PomodoroCloseSpec {
            raw: "x1*".to_string(),
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
            "x1* (in progress 1 · parked the remaining links)"
        );

        let bare_complete = PomodoroCloseSpec {
            raw: "x!".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: true,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(format_pomodoro_close(&bare_complete), "x! (complete all)");
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
