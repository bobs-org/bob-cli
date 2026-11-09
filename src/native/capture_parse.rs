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
        DiagnosticRewrite, EditorGlobalDestination, EditorItemParse,
        EditorMode, EditorParseOptions, EditorTarget, ListContext, MoveBullet,
        MoveChoiceBullet, Need, PomodoroAdjustSpec, PomodoroCloseSpec,
        PomodoroMergeSpec, PomodoroPastSpec, PomodoroShiftSpec,
        PomodoroSplitSpec, PomodoroStartSpec, ProjectTaskId, RelationDirection,
        RelationEntry, ResolvedTypeName, Severity, Span,
    },
    capture_links, capture_task_wikilinks,
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
    let prefix = "%t ";
    let draft = format!("{prefix}{text}\n\n");
    let text_start = prefix.len();
    let text_end = text_start + text.len();
    let options = EditorParseOptions {
        url_routing: None,
        has_global_destination: false,
        record_policy: None,
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
            merges_source: span.merges_source,
            offers_source: span.offers_source,
            project_note: span.project_note,
            title: span.title.clone(),
            key: span.key.clone(),
            directive: span.directive,
            count: span.count,
            gmail: None,
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
    // One memoized record route policy per invocation; ordinary drafts
    // skip config and filesystem reads via lexical preflight.
    let record_policy = super::capture_record::record_policy_for_draft(
        &super::env::bob_dir(),
        None,
        &raw_text,
    );
    let options = EditorParseOptions {
        url_routing: policy.as_ref(),
        has_global_destination: false,
        record_policy: Some(&record_policy),
    };
    let mut result = CaptureParseResult::new(raw_text, &options);
    attach_gmail_spans(&mut result);
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
--bob-dir. A '%c' directive still succeeds without reading the clipboard.\n\n\
Success JSON reports `grammar: 20` for the capture-language revision while \
`schema_version` stays 1. Gmail URLs add `gmail_url` spans with offline \
`gmail: {verdict, date_hint?}`, decoded thread `key`, and saved-label `title`. \
No Gmail calls are made.\n\n\
A lone task-list header `@route` with no section, bullets, or other text \
reports mode `note_pull`; `@route` with bullets remains a task list, and a \
lone `@route#section` remains an empty-list refusal.\n\n\
Configured record creation claims add a `record` object to the first item \
and its `items[]` row. It reports the selected type, destination, required \
fields, defaults, and field values; incomplete titles keep schema metadata \
and need `record_title`. Field `range` is present for one typed value, while \
`value_ranges[]` preserves separate source spans for repeated values.\n\n\
Adjacent `@route+id` target markers at the start or end of one line fill one \
route slot. Capture-parse keeps one item and adds `targets[]` for runs; \
duplicate task identities and targets at opposite edges report the coded \
diagnostics `duplicate_capture_target` and `split_capture_targets`.\n\n\
It reports the normalized body, the overall capture mode, the resolved route, \
section, and block ID, which parts a picker still has to supply, an optional \
global_destination object for a @@<route> \
declaration token anywhere in the draft, ordered blank-line-separated item summaries and ranges after \
inheritance, the UTF-8 byte spans of every recognized token (including a \
`task_type` span for `#<type>` markers), Obsidian wikilink \
component spans, every authored sub-bullet's normalized body plus depth, \
an optional pomodoro_start object for a `@<route>:<block-id>[#<name>]=<X>` \
start suffix (including the short `=name` suffix, e.g. `@work:outline=bugs`), an optional pomodoro_adjust object for a whole-item `+[N]`/`-[N]` \
adjustment, an optional pomodoro_shift object for a whole-item `++[N]`/`--[N]` \
shift, an optional pomodoro_split object for a whole-item `=/` split, an optional pomodoro_merge object for a whole-item `=*` merge, an optional pomodoro_close object for a `=x` link close suffix or \
whole-item close, and structured diagnostics. \
An `x=` item adds a text-only `pomodoro_past` object (the typed name, \
blocks, repair flag, optional segment suffix, stack position with optional \
pin and lead blocks, and existing close spec) plus `pomodoro_past`, \
`pomodoro_past_repair`, and `pomodoro_past_segment` spans. A run of `x=` \
items hangs from one place: a start suffix (`~`, `~n`) goes on its first \
item and an end suffix (`~n+`, `~~`) on its last. \
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
all report mode 'incomplete' plus what they still need. A free-standing '#' \
at an item edge reports mode 'incomplete' needing 'task_type', except a \
first-level bullet start (`- #`) which names a drawer (`needs: [\"drawer\"]` \
or `[\"drawer_notes\"]`). '#' plus a \
digit, '##…', '#-…', mid-line '#', and glued hashes stay ordinary prose. A valid `=<X>` start \
suffix on a `@<route>:<block-id>[#<name>]` marker reports mode \
'pomodoro_task' with a `pomodoro_start` object (`raw` plus 5-minute \
`duration_units`, earlier `offset_units`, and nonzero `later_units`) and a `pomodoro_start` span covering the \
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
covering only the operator token. A whole-item `=/` split (for example `=/` splits the current merged Pomodoro, `=/build` splits the merged entry whose base is build, `=/build#fix` splits only FIX out) reports mode 'pomodoro_split' with a `pomodoro_split` object (`raw`, optional `base`, and ordered `selectors`) plus a `pomodoro_split` span over `=/` and `pomodoro_name` spans over the base and each selector; a trailing `#` needs `pomodoro_split_type`. A whole-item `=*` merge (for example `=*#fix` merges FIX into the current Pomodoro, `=*build#fix` merges FIX into BUILD) reports mode 'pomodoro_merge' with a `pomodoro_merge` object (`raw`, optional `src`, and ordered `dests`) plus a `pomodoro_merge` span over `=*` and `pomodoro_name` spans over the source and each dest; a dest-less token merges the next planned Pomodoro below the source, while only a trailing `#` needs `pomodoro_merge_dest`. Digit and marker `=*` forms (`=*1`, `=*!2`, `=**`) stay retired-close teaching errors. A `@@` declaration still routes ordinary \
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
starts 25 minutes 10 minutes earlier, `=+2` starts 25 minutes 10 minutes later, \
and `=3+1` starts 15 minutes 5 minutes later) reports mode 'pomodoro_start' \
with a `pomodoro_start` object (`raw` excludes `=`, plus 5-minute \
`duration_units`, earlier `offset_units`, nonzero `later_units`, and the additive `drop` list) and a \
`pomodoro_start` span covering the `=` and leading X, plus a separate span \
for trailing X; a `~<K>` drop part adds a `pomodoro_start_drop` span covering \
`~<K>` including the `~`. The human \
`start` line reads `=3bugs~1,3 (15m, offset 0u · drop 1, 3)`. Every claimed \
doubled start (`==`, `==3#deep-work`, \
`==#`, `==~`, `==#deep-work=3`) reports mode 'pomodoro_start' with an \
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
`=3bugs~1`) starts the next session without those queued Task Links; a \
dangling `~`/`,` (`=~`, `=~2,`) reports mode 'incomplete' needing \
`pomodoro_start_task` with the partial spec, the spans typed so far, and \
one `interactive_placeholder` span over the separator, and every malformed \
list (a `0`, a duplicate, a second `~`, a `!`, a `#name` after the list, a \
bad character, an oversized number, or a space inside the list, which gets \
the no-spaces hint) reports `invalid_pomodoro_start` on the precise range. \
A named whole-item start (`=bugs`, `=3bugs`, `=bugs3`, `=3#deep-work`) starts that Pomodoro: \
`section` carries the typed name with a `pomodoro_name` span over the name \
bytes only (the `#` is in no span); leading `=<X>` and trailing X each have \
a separate `pomodoro_start` span. Bare names start with an ASCII letter, have \
no `-`, do not end in an ASCII digit, and cannot be close-shaped; `#` is \
only needed for names a bare spelling cannot express, such as `deep-work`, \
`2fa`, or `x2`. `=<X>#` with no name reports mode 'incomplete' needing \
`pomodoro_name` with an `interactive_placeholder` span over `#` and the partial \
`pomodoro_start` spec, and `x#name` reports mode 'pomodoro_close' with an \
`invalid_pomodoro_close` diagnostic over `#name`. Retired timing separators \
and redundant `#` prefixes report `invalid_pomodoro_start` with a teaching \
rewrite; a dashed bare name such as \
`=deep-work` teaches `=#deep-work`. A trailing timing follows a bare name \
(`=bugs3`); timing for a `#` name stays before it (`=3#deep-work`). \
A whole-item \
`x[<N>][*<P>][!<M>][~<K>][@<N>]` close (lowercase `x` only, with `*`, `!`, `~`, and `@` in \
any order; a capitalized `X…` stays prose) reports mode 'pomodoro_close' with a `pomodoro_close` object \
(`raw` exactly as typed plus the additive `in_progress` list, null when no `<N>` was typed, \
the `park` list, the `complete` list, the `drop` list, the additive `park_all` and `complete_all` flags, and the `log` entries (`index`, \
`text`, `details`, and an `all` flag on unexpanded star bullets) in typed order (`index` is omitted for unnumbered bullets under a close with no numbered lists or with wildcard intent, and for star bullets, which `bob capture` resolves after staging the running session lineup); and spans covering the `x` token (`pomodoro_close`), the `<N>` list \
including its commas (`pomodoro_close_in_progress`), the `*<P>` list \
including the `*` (`pomodoro_close_park`), the `!<M>` list \
including the `!` (`pomodoro_close_complete`), the `~<K>` list including \
the `~` (`pomodoro_close_drop`), and each numbered or star entry index \
(`pomodoro_close_log_index`; positional entries get no index span, a star bullet's `*` gets one, and entry text \
renders as neutral prose but keeps \
its wikilink spans); the human `close` line reads \
`x1*2!3~4 (in progress 1 · parked 2 · complete 3 · drop 4 · defer the rest)`, with \
`in progress none` for `x0`, `park all` for `x*`, `complete all` for `x!`, `park all · complete 2` for `x*!2`, `in progress 1 · park all` for `x1*`, and a bare `x` for a plain close, and \
`log 2 'wired the lexer' (+1 detail)` for typed entries with details \
(`log 'wired the lexer'` with no number for entries resolved at execution, `* 'code review'` for a star bullet). \
One entry may sit on the close line itself (`bob capture-parse -f json -- \
'x wired it'`): it logs to the first task the close works, or to the \
first listed worked task when numbered lists are typed. Under wildcard intent, the default index is \
omitted and Bob resolves it to the first eligible top-level worked Task Link. The item `body` stays the close token \
(`x`), the spec `log` carries the entry with an index only when lexically resolved, an \
explicit number gets a `pomodoro_close_log_index` span on the close line \
while the default gets none, and entry text keeps its wikilink spans. A \
dangling inline number (`bob capture-parse -f json -- 'x 2'`) reports mode \
'incomplete' needing `pomodoro_close_log_text` with the partial spec and an \
`interactive_placeholder` span over the number instead of its index span. \
Several entries still use child bullets below the close (`- [<n>] <text>`, `- * <text>`, with \
two-space `  - <detail>` details nesting under their entry): bullets are numbered \
all or none except star bullets, which never fix the list's kind, and unnumbered bullets log in order to the close's worked tasks \
(one worked task takes them all, otherwise bullet `i` logs to worked task `i`); \
a star bullet logs to every remaining worked task; \
a leading number is always a task number, so `- 2 bugs fixed` names task 2. Only the \
first token is an index and every backslash stays literal. A \
dangling bullet (`- 1` or `- *`) reports mode 'incomplete' needing \
`pomodoro_close_log_text` with the partial spec (lists plus every complete \
entry with its details), the spans typed so far, and one \
`interactive_placeholder` span over each dangling number or `*` instead of its \
index span. A mixed-numbering bullet, too many unnumbered bullets, a close \
that works no task, a star bullet with no task left, a non-loggable index, a bad number, a \
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
`=x…` and `=!…` whole-item spellings, plus digit and marker `=*` forms (`=*1`, `=*!2`, `=**`), report 'pomodoro_close' plus an \
`invalid_pomodoro_close` diagnostic that names the bare `x…` rewrite. On \
link items the `=x…` suffix (the `=` separates it from the block ID) spans the same four span kinds after the route \
and block-ID spans: `@r:id=x1!2` and `^r:id=x1` stay 'pomodoro_link' (or \
'pomodoro_task' with body text) and carry the spec, `^r:id=x1,` reports \
'incomplete' needing `pomodoro_close_task`, while `#name=x…`, \
`s:<N>`/`$<N>`/`%` conflicts, project-note `=x`, the retired `=*`/`=!` link aliases, and malformed lists \
report `invalid_pomodoro_close` on the conflicting component or the precise \
list range. A `@@` declaration never applies to close, `=`/`=<X>`, or \
`=name` items, and neither is ever rewritten. \
A whole-item `x=` past (`x=` starts now, `x` stops now, `x=` records what already happened: everything before the second `=` is time, everything after it is tasks) is lowercase only and must start an item or sit inside a same-line session chain; `X=3` and mid-body `Plan x=3` stay prose, while every `x=` token is claimed as valid, invalid, or unfinished, never prose. It reports mode 'pomodoro_past' with `section` carrying the typed name and a `pomodoro_past` object (`raw` exactly as typed, `name` (null when unnamed), `name_is_literal`, `blocks` (default 5), `minutes` (`blocks` times 5), `repair`, optional `segment` (`raw`, `kind` `tail`/`index`/`head`, optional 1-99 `index`, `edge` `start`/`end`), `stack` (`index` 1-based in typed order, `size`, `offset_blocks` summed from later members, plus `pin` `start`/`end` and `lead_blocks` summed from earlier members for pinned runs), and `close` in the existing close-spec shape including `log`). An optional segment suffix closes `E` (`x=~` starts where your last Pomodoro ended, `x=~n` starts at segment `n`, `x=~n+` ends at segment `n`, `x=~~` ends where your first Pomodoro starts); a run of `x=` items hangs from one place, so a start suffix goes on its first item and an end suffix on its last, and any other placement reports `invalid_pomodoro_past`. Names follow the `=name` rules: bare names need an ASCII letter, contain no `-`, do not end in a digit, and must not be close-shaped (digit-led needs `#`: `x=#2fa`; `x=2fa` starts FA for 10 minutes); a bare name takes one glued count on either side (`x=3fix`, `x=fix3`), and bytes `=name` rejects report its `Pomodoro name` wording; unquoted names end at `!`, `=`, `@`, `~`, `*`, `-`, and whitespace, while `#\"literal\"` quotes (straight or curly) canonicalize like `=#\"…\"`, match by key equality, and run through the closing quote (so `!`/`=` inside quotes are name content, not delimiters). The repair `!` binds to `E`, and everything after the second `=` goes to the close lexer unchanged (including `x=3=1,` needing `pomodoro_close_task` and `x= 2` needing `pomodoro_close_log_text`); an `x=` token counts as a close for chain ownership, inline entries, and child bullets. Spans cover `x=` plus a leading count (`pomodoro_past`), a trailing count (`pomodoro_past`), the name only (`pomodoro_name`), the suffix bytes (`pomodoro_past_segment`), the repair `!` (`pomodoro_past_repair`), the second `=` (`pomodoro_close`), plus the existing close sub-spans and `pomodoro_close_log_index` entries. Malformed `E` reports `invalid_pomodoro_past` (usage error from `bob capture`): past records have no offsets; a second count, zero count, close-shaped bare name, dashed bare name, retired `@` separator, redundant `#` prefix, malformed or misplaced segment suffix, or a second suffix in one run gets a teaching rewrite. Drop, park, and completion modifiers belong after a second `=`; `x=` cannot split or merge. Every rewrite preserves the typed count, name, repair `!`, and close part; oversized counts reuse the start `too large` wording; `x=#`/`x=3#`/`x=#\"DEEP` need `pomodoro_name`. \
A '@^id@' marker already carries the project-note intent: it reports mode \
'project_note' with a 'route' need until the \
route is typed. A '@:id@' marker is the offer form: it reports mode \
'pomodoro_project_note' with a 'route' need until the route is typed, and \
every marker span carries the 'offer' project-note modifier with a \
'project_note_offer_marker' seal span. A completed seal offers no picker and \
carries no needs; a trailing '#' opens Pomodoro-name completion. A missing ID \
or Pomodoro name keeps the offer mode and reports 'pomodoro_id' or \
'pomodoro_name' in 'needs', and `bob capture` refuses a route-less sealed \
marker with the Pomodoro route error before writing anything. A '@^id+' or '@:id+' marker is the retired project-note form and \
reports a `retired_project_note_marker` diagnostic. In a project-note item, a trailing \
` :id` / ` ^id` word on a first-level task bullet names that task: ` :id` additionally makes it Next and \
links it into the Pomodoro, reporting mode 'pomodoro_project_note' with a `project_task_link_marker` span over \
the `:` and a `project_task_block_id` span over the ID (` ^id` spans only the ID). Stripped bodies appear in \
`sub_bullets` with a parallel `sub_bullet_task_ids` array of `null` or `{\"block_id\", \"link\"}` entries, and \
`section` carries the marker's Pomodoro name. Misplaced, invalid, reserved, empty, or checkbox-carrying IDs report \
`misplaced_project_task_id` or `invalid_project_task_id`, a repeated ID reports `duplicate_project_task_id`, and a \
`#pomodoro` name with no ` :` task reports `unused_project_note_pomodoro` (or the offer wording when the marker is sealed with ':'). A lone trailing `:` or `^` is an unfinished \
ID: mode 'incomplete' needing `block_id` with an `interactive_placeholder` span over the sigil and no diagnostic. In a sealed project-note item, a first-level bullet holding only `^id` or `:id` moves that open task from the parent note into this slot instead: `:id` additionally makes it Next and links it, reporting mode 'pomodoro_project_note' with a `project_task_move_marker` span over the `^` (`project_task_move_link_marker` for `:`) and a `project_task_move_block_id` span over the ID. Move bullets appear with empty bodies in `sub_bullets` and a parallel `sub_bullet_moves` array of `null` or `{\"sigil\", \"block_id\", \"range\"}` entries (`range` covers the sigil plus ID, and `block_id` is null while unfinished). A lone `^` or `:` filling its first-level bullet is an unfinished move: mode 'incomplete' needing `project_task_move` with both the marker and an `interactive_placeholder` span over the sigil and no diagnostic. Move IDs share a namespace with authored task IDs: a repeated move reports `move_bullet_duplicate`, a move matching an authored ID reports `move_bullet_id_collision`, and words, markers, checkboxes, nested bullets, a cross-note `^route:id` shape, or `^prj` after the token reports its matching `move_bullet_*` diagnostic. An exact `- ^^` choice marker stays complete with a `project_task_move_choice_marker` span over both carets, an empty body, and a parallel `sub_bullet_move_choices` entry; its `sub_bullet_moves` entry stays null. \
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
Pomodoro adjustment in parsing. On a transfer line, terminal `+` or `+query` \
reports the same `task_parent` need; capture refuses it until a merge target is selected. \
An `&note:block-id` modifier names a prerequisite task link \
(`&projects/foo:bar`; quote notes with spaces: `&\"Shopping List\":bar`): \
one or more modifiers lead or trail the parent line (interleaving with \
destination, schedule, priority, and `%` directives in either order) or \
close an authored child line, and each adds a per-item `dependencies` entry \
(`raw`, decoded `note` and `block_id`, plus its sigil-inclusive `range`) \
with `dependency_sigil`, `dependency_note`, and `dependency_block_id` \
spans. Body text keeps its established mode and gains a `dependency_target` \
(`new_task` for a task this capture would create); a bare `&note:block-id` \
with an explicit `@note+task-id` (or the narrow bare `@note:id` / \
`^note:id` aliases, leading or trailing) reports mode 'task_dependency' \
with an `existing_task` target. A bare existing owner with `$<N>`, \
`s:<N>`, `%c`, or child bullets reports a `relationship_needs_text` \
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
spans with `cancels_source` and `completes_source`; configured merge labels \
mark them with `merges_source`, and configured successor labels mark them \
with `offers_source`. Relations never block, \
consume, promote, or schedule anything. The retired \
'@route::...' spelling is a diagnostic directing users to '@route^...'; \
it is not an incomplete Pomodoro marker. A `%n` directive reports mode \
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
            "Examples:\n  bob capture-parse --format json -- 'Call bank @cash+'\n  bob capture-parse 'Call bank @Cash+'\n  bob capture-parse -f json -- 'jot idea @notes#Ideas'\n  bob capture-parse -f json -- 'Postgres 17 minimum @foo+bar#req'\n  bob capture-parse -f json -- '@cash+goog-exit'\n  bob capture-parse -f json -- '+5'\n  bob capture-parse -f json -- '-2'\n  bob capture-parse -f json -- '++3'\n  bob capture-parse -f json -- '--'\n  bob capture-parse -f json -- '+'\n  printf '++3\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  printf '+5\\n\\nCall bank @Cash+\\n' | bob capture-parse -f json\n  echo 'Do work @dev^focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123' | bob capture-parse -f json\n  echo 'Do work @dev:focus-123#' | bob capture-parse -f json\n  printf '@@foo\\nFirst task\\n\\nSecond task @bar\\n' | bob capture-parse -f json\n  printf 'Parent\\n- first child\\n\\nSecond @work\\n' | bob capture-parse\n  bob capture-parse -f json -- 'x'\n  bob capture-parse -f json -- 'x1,3!2'\n  bob capture-parse -f json -- 'x1~2'\n  bob capture-parse -f json -- 'x0!2'\n  printf 'x\\n- 1 wired the lexer\\n' | bob capture-parse -f json\n  printf 'x\\n- 1\\n' | bob capture-parse -f json\n  printf 'x =\\n- 1 wired it\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '='\n  bob capture-parse -f json -- '=3'\n  bob capture-parse -f json -- '=~2'\n  bob capture-parse -f json -- '=bugs'\n  bob capture-parse -f json -- '=3bugs~1'\n  bob capture-parse -f json -- 'x =~2'\n  bob capture-parse -f json -- '@r:id=x'\n  bob capture-parse -f json -- '^r:id=x1'\n  printf 'x\\n\\n=\\n' | bob capture-parse -f json\n  bob capture-parse -f json -- '+2 x'\n  bob capture-parse -f json -- 'Buy Groceries! &foo:bar'\n  bob capture-parse -f json -- 'Ask Kelly *job:cleanup'\n  bob capture-parse -f json -- '&foo:bar @body+excercise'\n  bob capture-parse -f json -- '^r:id &foo:bar'\n  bob capture-parse -f json -- '&foo:bar +'\n  bob capture-parse -f json -- '&foo:bar'\n  bob capture-parse -f json -- 'Buy Groceries! &fo'\n  bob capture-parse -f json -- 'Fix trip ~work:foo'\n  bob capture-parse -f json -- 'Fix trip ~~work:foo|see'\n  bob capture-parse -f json -- '/fix'\n  bob capture-parse -f json -- 'Ramen Nagi @eat/'\n\nModes:\n  task, bullet, pomodoro_task, pomodoro_note, sub_bullet, task_toggle, project_note, pomodoro_project_note, pomodoro_adjust, pomodoro_shift, pomodoro_link, pomodoro_close, pomodoro_past, pomodoro_start, task_dependency, task_complete, record_note, ref, incomplete\n\nNeeds:\n  route, section, block_id, pomodoro_id, pomodoro_name, task, drawer, active_task, task_link, task_goto, task_parent, pomodoro_close_task, pomodoro_close_log_text, pomodoro_start_task, task_dependency, blocked_task, relation_target, relation_label, dependency_target, task_complete, task_type, record_title, record_value",
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
    grammar: u32,
    input: String,
    body: String,
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    /// The capture list the first item belongs to, when split from a lone
    /// `@route` / `@route#section` header. Additive: omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    list: Option<ListContext>,
    /// The header line's range. Present exactly when `list` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    header_range: Option<SourceRange>,
    /// Top-level `relation_grammar: 1` whenever the draft contains a `~`
    /// byte. Omitted otherwise, so drafts without relations stay
    /// byte-identical to today.
    #[serde(skip_serializing_if = "Option::is_none")]
    relation_grammar: Option<u8>,
    /// Additive record creation metadata for the first item. Omitted for
    /// ordinary task captures and reserved update spellings.
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<ParseRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_parent: Option<RefParentParse>,
    /// Whether the `%l` directive requested companion audio.
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
    /// Additive schema-version-1 field aligned one-to-one with
    /// `sub_bullets`: each entry is `null` or
    /// `{"sigil", "block_id", "range"}` (`block_id` is null while the
    /// move is unfinished). Emitted only when at least one entry is
    /// non-null, so every older input keeps its exact JSON shape.
    #[serde(skip_serializing_if = "all_moves_none")]
    sub_bullet_moves: Vec<Option<MoveBullet>>,
    /// Additive schema-version-1 field aligned one-to-one with
    /// `sub_bullets`: each entry is `null` or `{"range"}` for an exact
    /// `- ^^` choice marker. Emitted only when at least one entry is
    /// non-null, so every older input keeps its exact JSON shape.
    #[serde(skip_serializing_if = "all_choices_none")]
    sub_bullet_move_choices: Vec<Option<MoveChoiceBullet>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<CaptureParseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    template: Option<TemplateParse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    global_destination: Option<GlobalDestinationParse>,
    /// Validated additive `@<route>:<block-id>[#<name>]=<X>` start suffix:
    /// the raw `<X>` text plus 5-minute duration, earlier, and later units. Omitted
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
    /// Validated additive whole-item `=/` split spec. Omitted for every
    /// older input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_split: Option<PomodoroSplitSpec>,
    /// Validated additive whole-item `=*` merge spec. Omitted for every
    /// older input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_merge: Option<PomodoroMergeSpec>,
    /// Validated additive `=x` link close suffix: the typed `=x…` text as `raw`.
    /// Omitted for every older input, so schema version 1 is unchanged
    /// for inputs without a close. Purely lexical and never guesses
    /// current ledger times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_close: Option<PomodoroCloseSpec>,
    /// Validated whole-item `x=` past spec: `raw` is the trimmed past token
    /// exactly as typed, plus canonical `name`, `blocks`/`minutes`,
    /// `repair`, `stack`, and the existing close-spec `close`. Omitted for
    /// every older input. Purely lexical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_past: Option<PastWire>,
    /// Complete transfer syntax, omitted for ordinary items.
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer: Option<capture_language::EditorTransfer>,
    /// Complete merge-in clause, omitted for ordinary items.
    #[serde(skip_serializing_if = "Option::is_none")]
    merge_in: Option<capture_language::EditorMergeIn>,
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
    /// Additive target-run members for the first item, in source order.
    /// Omitted unless the first item is a multi-member run, so every
    /// older input keeps its exact JSON shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    targets: Vec<ParseTarget>,
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
    route: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer: Option<capture_language::EditorTransfer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    merge_in: Option<capture_language::EditorMergeIn>,
    needs: Vec<Need>,
    /// The capture list this bullet belongs to, when split from a lone
    /// `@route` / `@route#section` header. Additive: omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    list: Option<ListContext>,
    /// The header line's range. Present exactly when `list` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    header_range: Option<SourceRange>,
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
    #[serde(skip_serializing_if = "all_moves_none")]
    sub_bullet_moves: Vec<Option<MoveBullet>>,
    #[serde(skip_serializing_if = "all_choices_none")]
    sub_bullet_move_choices: Vec<Option<MoveChoiceBullet>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_start: Option<PomodoroStartSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_adjust: Option<PomodoroAdjustSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_shift: Option<PomodoroShiftSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_split: Option<PomodoroSplitSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_merge: Option<PomodoroMergeSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_close: Option<PomodoroCloseSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pomodoro_past: Option<PastWire>,
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
    /// Additive target-run members for this item, in source order.
    /// Omitted unless the item is a multi-member run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    targets: Vec<ParseTarget>,
    /// Additive record creation metadata for this item, when claimed.
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<ParseRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseRecord {
    action: &'static str,
    #[serde(rename = "type")]
    record_type: String,
    label: String,
    emoji: Option<String>,
    dir: String,
    path: Option<String>,
    stem: Option<String>,
    stem_source: Option<&'static str>,
    title: String,
    missing: Vec<String>,
    notes: usize,
    fields: Vec<ParseRecordField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseRecordField {
    key: String,
    label: String,
    kind: &'static str,
    required: bool,
    list: bool,
    state: &'static str,
    value: Option<serde_json::Value>,
    display: Option<String>,
    source: Option<&'static str>,
    range: Option<SourceRange>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    value_ranges: Vec<SourceRange>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    choices: Vec<ParseRecordChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseRecordChoice {
    value: String,
    label: String,
}

/// Wire projection of a validated whole-item `x=` past spec. `minutes` is
/// `blocks * 5`; `blocks_typed` stays internal and is never serialized.
/// `name` is `null` when unnamed. `segment` is the optional segment suffix
/// (`~`/`~n`/`~n+`/`~~`), omitted when none. `stack` carries the text-only
/// run position (`index` 1-based, `size`, `offset_blocks` summed from later
/// members, plus `pin`/`lead_blocks` for pinned runs). `close` reuses the
/// existing close-spec shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PastWire {
    raw: String,
    name: Option<String>,
    name_is_literal: bool,
    blocks: u32,
    minutes: u64,
    repair: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment: Option<PomodoroPastSpecSegmentWire>,
    stack: PastStackWire,
    close: PomodoroCloseSpec,
}

/// Wire projection of [`PomodoroPastSpec::segment`]: `raw` plus `kind`
/// (`tail`/`index`/`head`), optional 1–99 `index`, and `edge`
/// (`start`/`end`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PomodoroPastSpecSegmentWire {
    raw: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<u32>,
    edge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PastStackWire {
    index: u32,
    size: u32,
    offset_blocks: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lead_blocks: Option<u32>,
}

fn past_wire(spec: &PomodoroPastSpec) -> PastWire {
    PastWire {
        raw: spec.raw.clone(),
        name: spec.name.clone(),
        name_is_literal: spec.name_is_literal,
        blocks: spec.blocks,
        minutes: spec.blocks as u64 * 5,
        repair: spec.repair,
        segment: spec.segment.as_ref().map(|segment| {
            let (kind, index) = match segment.kind {
                capture_language::PastSegmentKind::Tail => {
                    ("tail".to_string(), None)
                }
                capture_language::PastSegmentKind::Head => {
                    ("head".to_string(), None)
                }
                capture_language::PastSegmentKind::Index => {
                    ("index".to_string(), segment.index)
                }
            };
            let edge = match segment.edge {
                capture_language::PastEdge::Start => "start".to_string(),
                capture_language::PastEdge::End => "end".to_string(),
            };
            PomodoroPastSpecSegmentWire {
                raw: segment.raw.clone(),
                kind,
                index,
                edge,
            }
        }),
        stack: PastStackWire {
            index: spec.stack.index,
            size: spec.stack.size,
            offset_blocks: spec.stack.offset_blocks,
            pin: spec.stack.pin.map(|pin| match pin {
                capture_language::PastEdge::Start => "start".to_string(),
                capture_language::PastEdge::End => "end".to_string(),
            }),
            lead_blocks: spec.stack.lead_blocks,
        },
        close: spec.close.clone(),
    }
}

/// One `@route+id` target member: exact source range, marker text, and the
/// member's resolved mode/route. Additive schema-version-1 field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseTarget {
    start: usize,
    end: usize,
    marker: String,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<String>,
}

fn parse_targets(targets: &[EditorTarget]) -> Vec<ParseTarget> {
    targets
        .iter()
        .map(|target| ParseTarget {
            start: target.start,
            end: target.end,
            marker: target.marker.clone(),
            mode: target.mode.label().to_string(),
            route: target.route.clone(),
            block_id: target.block_id.clone(),
            section: target.section.clone(),
        })
        .collect()
}

fn is_no_parse_project_note(
    form: &Option<super::capture_language::ProjectNoteForm>,
) -> bool {
    form.is_none()
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
    /// True when a `relation_label` span's label is configured to merge
    /// its source into the target task.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) merges_source: bool,
    /// True when a `relation_label` span's label is a configured successor
    /// label (`relations.successor_labels`). Omitted otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) offers_source: bool,
    /// Sealed project-note modifier (`"create"` or `"offer"`). Present on
    /// every span of a project-note marker token except the Pomodoro name.
    #[serde(default, skip_serializing_if = "is_no_parse_project_note")]
    pub(crate) project_note: Option<super::capture_language::ProjectNoteForm>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    /// Named directive (`"clip"`, `"note"`, `"listen"`). Present on a
    /// complete `%c` / `%c<N>`, `%n`, or `%l` token; omitted for a bare `%`
    /// and for unknown tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) directive: Option<&'static str>,
    /// History count for `%c<N>` when N ≥ 2. Omitted for `%c` / `%c1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gmail: Option<super::gmail::capture::SpanMetadata>,
}

fn attach_gmail_spans(result: &mut CaptureParseResult) {
    let matches = super::gmail::ids::find_gmail_urls(&result.input);
    if matches.is_empty() {
        return;
    }
    // Record regions stay opaque: a URL inside a record field value (or
    // any other record span) is validated by its schema kind, never by
    // Gmail rewriting.
    let record_ranges: Vec<(usize, usize)> = result
        .spans
        .iter()
        .filter(|span| {
            matches!(
                span.kind,
                capture_language::SpanKind::RecordRoute
                    | capture_language::SpanKind::RecordStem
                    | capture_language::SpanKind::RecordFieldKey
                    | capture_language::SpanKind::RecordFieldValue
            )
        })
        .map(|span| (span.start, span.end))
        .collect();
    let context =
        super::gmail::capture::OfflineContext::load(&super::env::bob_dir());
    for found in matches {
        let preview = context.preview(&found);
        // Underline the URL itself, keeping authored Markdown labels outside it.
        let start = found.start
            + result.input[found.start..found.end]
                .find(&found.url)
                .unwrap_or(0);
        let end = start + found.url.len();
        if record_ranges
            .iter()
            .any(|range| range.0 < end && start < range.1)
        {
            continue;
        }
        let mut span = wire_spans(vec![Span {
            start,
            end,
            kind: capture_language::SpanKind::GmailUrl,
            project_note: None,
            title: None,
            key: None,
        }])
        .remove(0);
        span.key = preview.thread_id;
        span.title = (preview.verdict == "known").then(|| {
            preview
                .message
                .trim_end_matches(" · already saved")
                .to_string()
        });
        span.gmail = Some(super::gmail::capture::SpanMetadata {
            verdict: preview.verdict,
            date_hint: preview.date_hint,
        });
        result
            .spans
            .retain(|existing| existing.end <= start || existing.start >= end);
        result.spans.push(span);
    }
    result.spans.sort_by_key(|span| (span.start, span.end));
}

fn is_theme_name_span(span: &CaptureParseSpan) -> bool {
    matches!(
        span.kind,
        capture_language::SpanKind::PomodoroName
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
    directive: TemplateDirectiveParse,
    range: SourceRange,
    line_start: usize,
    line_end: usize,
    field_count: usize,
    rows: Vec<TemplateRowParse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TemplateDirectiveParse {
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    needs: Vec<Need>,
    fields: Vec<TemplateFieldParse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<ParseRecord>,
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
        let analysis = capture_language::template::analyze_for_capture(
            &input,
            options.record_policy,
        );
        if analysis.directive.is_some() {
            return Self::new_template(input, options, analysis);
        }

        let parse = capture_language::parse_for_editor_with(&input, options);
        let record_plans = super::capture_record::resolve_record_intents(
            &parse.record_intents,
            options.record_policy,
        );
        let mut spans = wire_spans(merge_spans(
            parse.spans,
            capture_links::wikilink_spans(&input),
        ));
        stamp_directive_spans(&mut spans, &input);
        fill_span_gaps(
            &mut spans,
            capture_task_wikilinks::wikilink_task_spans(&input),
        );
        let close_policy = load_close_policy();
        stamp_cancel_spans(&mut spans, &input, &close_policy);
        stamp_complete_spans(&mut spans, &input, &close_policy);
        stamp_merge_spans(&mut spans, &input, &close_policy);
        stamp_successor_spans(&mut spans, &input, &close_policy);
        let items = parse_items(&parse.items, &parse.transfers, &record_plans);
        let record = parse
            .items
            .first()
            .and_then(|item| record_entry(&record_plans, item.index))
            .map(parse_record);
        let transfer = parse.items.first().and_then(|item| {
            parse
                .transfers
                .iter()
                .find(|transfer| transfer.index == item.index)
                .map(|transfer| transfer.transfer.clone())
        });
        let merge_in =
            parse.items.first().and_then(|item| item.merge_in.clone());
        let ref_parent = ref_parent_for(
            parse.mode,
            parse.route.as_deref(),
            parse.items.first().map(|item| item.has_local_destination),
        );
        let ref_listen = parse.ref_listen;
        let sub_bullets = parse.sub_bullets;
        let sub_bullet_depths = sub_bullet_depths(&sub_bullets);
        let sub_bullet_task_ids = sub_bullet_task_ids(&sub_bullets);
        let sub_bullet_moves = sub_bullet_moves(&sub_bullets);
        let sub_bullet_move_choices = sub_bullet_move_choices(&sub_bullets);
        let mut diagnostics = Vec::new();
        if let Some(warning) =
            missing_template_directive_hint(&input, &parse.items)
        {
            diagnostics.push(warning);
        }
        diagnostics.extend(
            analysis
                .issues
                .iter()
                .filter(|issue| issue.code() == "template_directive_misplaced")
                .map(template_issue_diagnostic),
        );
        diagnostics.extend(record_parse_diagnostics(&input, &record_plans));
        diagnostics.extend(collection_route_diagnostics(
            &input,
            &parse.items,
            options.record_policy,
        ));
        diagnostics.extend(parse.diagnostics.into_iter().filter(
            |diagnostic| {
                !analysis.issues.iter().any(|issue| {
                    issue.code() == "template_directive_misplaced"
                        && ranges_overlap(
                            diagnostic.range,
                            Some((issue.range().start, issue.range().end)),
                        )
                })
            },
        ));
        let relation_grammar = relation_grammar_for(&input);
        let relations = parse_relations(&parse.relations);
        let top_list = items.first().and_then(|item| item.list.clone());
        let top_header_range = top_list.as_ref().map(|list| SourceRange {
            start: list.header_start,
            end: list.header_end,
        });
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            input,
            body: parse.body,
            mode: parse.mode,
            route: parse.route,
            section: parse.section,
            block_id: parse.block_id,
            needs: {
                let mut needs = parse.needs;
                if record_plans
                    .first()
                    .is_some_and(|entry| !entry.missing.is_empty())
                    && !needs.contains(&Need::RecordValue)
                {
                    needs.push(Need::RecordValue);
                }
                needs
            },
            list: top_list,
            header_range: top_header_range,
            relation_grammar,
            record,
            ref_parent,
            ref_listen,
            spans,
            diagnostics,
            sub_bullets: sub_bullet_bodies(&sub_bullets),
            sub_bullet_depths,
            sub_bullet_task_ids,
            sub_bullet_moves,
            sub_bullet_move_choices,
            items,
            template: None,
            global_destination: parse
                .global_destination
                .as_ref()
                .map(global_destination_parse),
            pomodoro_start: parse.pomodoro_start,
            pomodoro_adjust: parse.pomodoro_adjust,
            pomodoro_shift: parse.pomodoro_shift,
            pomodoro_split: parse.pomodoro_split,
            pomodoro_merge: parse.pomodoro_merge,
            pomodoro_close: parse.pomodoro_close,
            pomodoro_past: parse.pomodoro_past.as_ref().map(past_wire),
            transfer,
            merge_in,
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
            targets: parse_targets(&parse.targets),
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

        let (directive_range, directive_line) = analysis
            .directive
            .as_ref()
            .expect("template parse has a directive");
        let (template_range, line_start, line_end) = analysis
            .block
            .as_ref()
            .map(|block| {
                (block.range.clone(), block.line_start, block.line_end)
            })
            .unwrap_or_else(|| {
                (directive_range.clone(), *directive_line, *directive_line)
            });

        let expanded_rows = capture_language::template::expand_rows_for_editor(
            &input, &analysis,
        );
        let row_parses = expanded_rows
            .iter()
            .map(|instance| {
                instance.as_ref().map(|instance| {
                    capture_language::parse_for_editor_with(
                        &instance.text,
                        options,
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut record_intents = Vec::new();
        let mut record_rows = std::collections::HashMap::new();
        let mut record_index = 0usize;
        for (row_index, row_parse) in row_parses.iter().enumerate() {
            let Some(row_parse) = row_parse.as_ref() else {
                continue;
            };
            for (_, intent) in &row_parse.record_intents {
                record_index += 1;
                record_intents.push((record_index, intent.clone()));
                record_rows.insert(record_index, row_index);
            }
        }
        let record_entries = super::capture_record::resolve_record_intents(
            &record_intents,
            options.record_policy,
        );
        let mut records_by_row =
            vec![
                Vec::<super::capture_record::RecordParseEntry>::new();
                analysis.rows.len()
            ];
        for entry in record_entries {
            let Some(row_index) = record_rows.get(&entry.index).copied() else {
                continue;
            };
            let (Some(instance), Some(row)) = (
                expanded_rows.get(row_index).and_then(Option::as_ref),
                analysis.rows.get(row_index),
            ) else {
                continue;
            };
            records_by_row[row_index].push(map_template_record_entry(
                entry,
                instance,
                &row.fields,
                &row.range,
            ));
        }
        let mapped_record_entries =
            records_by_row.iter().flatten().cloned().collect::<Vec<_>>();
        let mut row_summaries = Vec::with_capacity(analysis.rows.len());
        let mut row_items = Vec::with_capacity(analysis.rows.len());
        let mut row_record_spans = Vec::new();
        let mut instance_diagnostics = Vec::new();
        let mut refusal_diagnostics = Vec::new();
        let mut incomplete_diagnostics = Vec::new();

        for (index, row) in analysis.rows.iter().enumerate() {
            let instance = expanded_rows.get(index).and_then(Option::as_ref);
            let row_parse = row_parses.get(index).and_then(Option::as_ref);
            let record_entry = records_by_row
                .get(index)
                .and_then(|entries| entries.first());
            let mut row_needs = row_parse
                .map(|parsed| parsed.needs.clone())
                .unwrap_or_default();
            if record_entry.is_some_and(|record| !record.missing.is_empty())
                && !row_needs.contains(&Need::RecordValue)
            {
                row_needs.push(Need::RecordValue);
            }
            if let (Some(instance), Some(row_parse)) =
                (instance, row_parse.as_ref())
            {
                row_record_spans.extend(
                    row_parse
                        .spans
                        .iter()
                        .filter(|span| {
                            matches!(
                                span.kind,
                                capture_language::SpanKind::RecordRoute
                                    | capture_language::SpanKind::RecordStem
                                    | capture_language::SpanKind::RecordFieldKey
                                    | capture_language::SpanKind::RecordFieldValue
                            )
                        })
                        .map(|span| {
                            let (start, end) = map_instance_range(
                                Some((span.start, span.end)),
                                instance,
                                &row.fields,
                                &row.range,
                            );
                            capture_language::Span {
                                start,
                                end,
                                ..span.clone()
                            }
                        }),
                );
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
                    mapped.rewrite =
                        diagnostic.rewrite.as_ref().and_then(|rewrite| {
                            map_instance_rewrite(rewrite, instance, &row.fields)
                        });
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
                    &row_parse.pomodoro_past,
                    &row_parse.items,
                    (row.range.start, row.range.end),
                    &instance.text,
                ) {
                    let mapped_range = map_instance_range(
                        diagnostic.range,
                        instance,
                        &row.fields,
                        &row.range,
                    );
                    let mapped_rewrite =
                        diagnostic.rewrite.as_ref().and_then(|rewrite| {
                            map_instance_rewrite(rewrite, instance, &row.fields)
                        });
                    refusal_diagnostics.push(Diagnostic {
                        range: Some(mapped_range),
                        rewrite: mapped_rewrite,
                        ..diagnostic
                    });
                }

                if !row_needs.is_empty()
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
                        rewrite: None,
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
                needs: row_needs.clone(),
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
                record: record_entry.map(parse_record),
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
                    needs: row_needs,
                    ref_parent: None,
                    ref_listen: row_parse.ref_listen,
                    sub_bullets: Vec::new(),
                    sub_bullet_depths: Vec::new(),
                    sub_bullet_task_ids: Vec::new(),
                    sub_bullet_moves: Vec::new(),
                    sub_bullet_move_choices: Vec::new(),
                    pomodoro_start: None,
                    pomodoro_adjust: None,
                    pomodoro_shift: None,
                    pomodoro_split: None,
                    pomodoro_merge: None,
                    pomodoro_close: None,
                    pomodoro_past: None,
                    transfer: None,
                    merge_in: None,
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    task_complete: None,
                    list: None,
                    header_range: None,
                    template_row: Some(template_row),
                    targets: Vec::new(),
                    record: record_entry.map(parse_record),
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
                    transfer: None,
                    merge_in: None,
                    needs: Vec::new(),
                    ref_parent: None,
                    ref_listen: false,
                    sub_bullets: Vec::new(),
                    sub_bullet_depths: Vec::new(),
                    sub_bullet_task_ids: Vec::new(),
                    sub_bullet_moves: Vec::new(),
                    sub_bullet_move_choices: Vec::new(),
                    pomodoro_start: None,
                    pomodoro_adjust: None,
                    pomodoro_shift: None,
                    pomodoro_split: None,
                    pomodoro_merge: None,
                    pomodoro_close: None,
                    pomodoro_past: None,
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    task_complete: None,
                    list: None,
                    header_range: None,
                    template_row: Some(summary.clone()),
                    targets: Vec::new(),
                    record: None,
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
        spans.extend(wire_spans(row_record_spans));
        spans.sort_by_key(|span| (span.start, span.end, span.kind.label()));
        spans.dedup_by(|left, right| {
            left.start == right.start
                && left.end == right.end
                && left.kind == right.kind
        });
        stamp_directive_spans(&mut spans, &input);
        fill_span_gaps(
            &mut spans,
            capture_task_wikilinks::wikilink_task_spans(&masked),
        );
        let close_policy = load_close_policy();
        stamp_cancel_spans(&mut spans, &masked, &close_policy);
        stamp_complete_spans(&mut spans, &masked, &close_policy);
        stamp_merge_spans(&mut spans, &masked, &close_policy);
        stamp_successor_spans(&mut spans, &masked, &close_policy);

        // A header in the template body is refused in execution
        // (`template_list`); surface the same code here so the template
        // view agrees. Masking can hide the header from the item parse, so
        // scan the definition lines exactly like the execution check, and
        // point at the header token in the original input.
        let mut diagnostics: Vec<Diagnostic> = analysis
            .block
            .as_ref()
            .and_then(|block| {
                let definitions = block.definition_lines(&input);
                block.lines.iter().zip(definitions.iter()).find_map(
                    |((_, range), definition)| {
                        capture_language::detect_list_header(
                            &capture_language::RawLine {
                                text: definition,
                                start: 0,
                                end: definition.len(),
                            },
                        )
                        .map(|_| {
                            let token = definition.trim();
                            (range.start, range.start + token.len())
                        })
                    },
                )
            })
            .map(|(start, end)| Diagnostic {
                severity: Severity::Error,
                code: "template_list",
                message: capture_language::template_list_error(),
                range: Some((start, end)),
                rewrite: None,
            })
            .into_iter()
            .collect();
        diagnostics.extend(
            analysis
                .issues
                .iter()
                .filter(|issue| issue.code() != "template_missing_rows")
                .map(template_issue_diagnostic),
        );

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
        diagnostics
            .extend(record_parse_diagnostics(&input, &mapped_record_entries));

        // Instance parses are the source of truth for clipboard: the masked
        // copy (`xxx %c`) reads as `%c` while a `URL %l` row
        // expands to a Listen reference. Only the session check runs here.
        if let Some(diagnostic) = refusal_diagnostic(
            &parsed.spans,
            parsed.mode,
            &parsed.pomodoro_start,
            &parsed.pomodoro_close,
            &parsed.pomodoro_past,
            &parsed.items,
            (template_range.start, template_range.end),
            &masked,
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
            .map(|item| wire_item(item, &parsed.transfers, &[]))
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

        let directive = TemplateDirectiveParse {
            range: SourceRange {
                start: directive_range.start,
                end: directive_range.end,
            },
            line: *directive_line,
        };
        let template = TemplateParse {
            directive,
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
        let sub_bullet_moves = sub_bullet_moves(&sub_bullets);
        let sub_bullet_move_choices = sub_bullet_move_choices(&sub_bullets);

        let relation_grammar = relation_grammar_for(&input);
        let relations = parse_relations(&parsed.relations);
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            grammar: capture_language::GRAMMAR_REVISION,
            input,
            body,
            mode: parsed.mode,
            route: parsed.route,
            section: parsed.section,
            block_id: parsed.block_id,
            needs,
            // Template rows never split into lists.
            list: None,
            header_range: None,
            relation_grammar,
            record: None,
            ref_parent,
            ref_listen: parsed.ref_listen,
            spans,
            diagnostics,
            sub_bullets: sub_bullet_bodies(&sub_bullets),
            sub_bullet_depths,
            sub_bullet_task_ids,
            sub_bullet_moves,
            sub_bullet_move_choices,
            items,
            template: Some(template),
            global_destination: None,
            pomodoro_start: parsed.pomodoro_start,
            pomodoro_adjust: parsed.pomodoro_adjust,
            pomodoro_shift: parsed.pomodoro_shift,
            pomodoro_split: parsed.pomodoro_split,
            pomodoro_merge: parsed.pomodoro_merge,
            pomodoro_close: parsed.pomodoro_close,
            pomodoro_past: parsed.pomodoro_past.as_ref().map(past_wire),
            transfer: None,
            merge_in: None,
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
            targets: parse_targets(&parsed.targets),
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
            merge_labels: Vec::new(),
            successor_labels: Vec::new(),
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

/// Mark `relation_label` spans whose label is configured to merge the
/// source into its target task. Omitted (false) everywhere else.
fn stamp_merge_spans(
    spans: &mut [CaptureParseSpan],
    input: &str,
    policy: &crate::native::config::RelationsConfig,
) {
    if policy.merge_labels.is_empty() {
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
        if policy.is_merge_label(
            &crate::native::relation_labels::relation_label_key(text),
        ) {
            span.merges_source = true;
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

/// Mark relation labels that offer the source task on a later parked or
/// done event. This is presentation-only; relation labels never block.
fn stamp_successor_spans(
    spans: &mut [CaptureParseSpan],
    input: &str,
    policy: &crate::native::config::RelationsConfig,
) {
    if policy.successor_labels.is_empty() {
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
        if policy.is_successor_label(
            &crate::native::relation_labels::relation_label_key(text),
        ) {
            span.offers_source = true;
        }
    }
}

fn stamp_directive_spans(spans: &mut [CaptureParseSpan], input: &str) {
    for span in spans {
        if span.kind != capture_language::SpanKind::Directive {
            continue;
        }
        let Some(token) = input.get(span.start..span.end) else {
            continue;
        };
        if token == "%n" {
            span.directive = Some("note");
            continue;
        }
        if token == "%l" {
            span.directive = Some("listen");
            continue;
        }
        if token == "%t" {
            span.directive = Some("template");
            continue;
        }
        let Some(rest) = token.strip_prefix("%c") else {
            continue;
        };
        span.directive = Some("clip");
        if rest.is_empty() {
            continue;
        }
        if let Ok(count) = rest.parse::<usize>()
            && count > 1
        {
            span.count = Some(count);
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
            merges_source: false,
            offers_source: false,
            project_note: span.project_note,
            title: span.title.clone(),
            key: span.key.clone(),
            directive: None,
            count: None,
            gmail: None,
        })
        .collect()
}

/// Add task-wikilink spans only where the already-resolved capture and
/// Obsidian-wikilink spans leave bytes uncovered. Unlike `merge_spans`, this
/// preserves the unoccupied parts of a long query around capture markers.
fn fill_span_gaps(spans: &mut Vec<CaptureParseSpan>, candidates: Vec<Span>) {
    let mut additions = Vec::new();
    for candidate in candidates {
        if candidate.start >= candidate.end {
            continue;
        }
        let mut covered = spans
            .iter()
            .filter(|span| {
                span.start < candidate.end && span.end > candidate.start
            })
            .map(|span| (span.start, span.end))
            .collect::<Vec<_>>();
        covered.sort_unstable();

        let mut cursor = candidate.start;
        for (start, end) in covered {
            if start > cursor {
                additions.push(CaptureParseSpan {
                    start: cursor,
                    end: start.min(candidate.end),
                    kind: candidate.kind,
                    field: None,
                    themes: Vec::new(),
                    cancels_source: false,
                    completes_source: false,
                    merges_source: false,
                    offers_source: false,
                    project_note: candidate.project_note,
                    title: candidate.title.clone(),
                    key: candidate.key.clone(),
                    directive: None,
                    count: None,
                    gmail: None,
                });
            }
            cursor = cursor.max(end);
            if cursor >= candidate.end {
                break;
            }
        }
        if cursor < candidate.end {
            additions.push(CaptureParseSpan {
                start: cursor,
                end: candidate.end,
                kind: candidate.kind,
                field: None,
                themes: Vec::new(),
                cancels_source: false,
                completes_source: false,
                merges_source: false,
                offers_source: false,
                project_note: candidate.project_note,
                title: candidate.title.clone(),
                key: candidate.key.clone(),
                directive: None,
                count: None,
                gmail: None,
            });
        }
        spans.append(&mut additions);
    }
    spans.sort_by_key(|span| (span.start, span.end));
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
        rewrite: None,
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
                    merges_source: false,
                    offers_source: false,
                    project_note: span.project_note,
                    title: span.title.clone(),
                    key: span.key.clone(),
                    directive: None,
                    count: None,
                    gmail: None,
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
                merges_source: false,
                offers_source: false,
                project_note: span.project_note,
                title: span.title.clone(),
                key: span.key.clone(),
                directive: None,
                count: None,
                gmail: None,
            });
        }
    }

    if let Some((range, _)) = analysis.directive.as_ref() {
        result.push(CaptureParseSpan {
            start: range.start,
            end: range.end,
            kind: SpanKind::Directive,
            field: None,
            themes: Vec::new(),
            cancels_source: false,
            completes_source: false,
            merges_source: false,
            offers_source: false,
            project_note: None,
            title: None,
            key: None,
            directive: None,
            count: None,
            gmail: None,
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
            merges_source: false,
            offers_source: false,
            project_note: None,
            title: None,
            key: None,
            directive: None,
            count: None,
            gmail: None,
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
            merges_source: false,
            offers_source: false,
            project_note: None,
            title: None,
            key: None,
            directive: None,
            count: None,
            gmail: None,
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

fn missing_template_directive_hint(
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
        code: "template_directive_missing",
        message:
            "add `%t` to this item to use it as a template for the lines below"
                .to_string(),
        range: Some((first.start, first.end)),
        rewrite: None,
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

fn map_template_record_entry(
    mut entry: super::capture_record::RecordParseEntry,
    instance: &capture_language::template::TemplateInstance,
    fields: &[capture_language::template::TemplateField],
    row_range: &std::ops::Range<usize>,
) -> super::capture_record::RecordParseEntry {
    let map = |range: (usize, usize)| {
        map_instance_range(Some(range), instance, fields, row_range)
    };
    entry.intent.route_range = map(entry.intent.route_range);
    entry.intent.title_range = entry.intent.title_range.map(map);
    entry.intent.stem_range = entry.intent.stem_range.map(map);
    for field in &mut entry.intent.fields {
        field.key_range = map(field.key_range);
        field.value_range = map(field.value_range);
    }
    for group in &mut entry.intent.notes {
        group.section_range = group.section_range.map(map);
        for line in &mut group.lines {
            map_template_record_note_line(line, &map);
        }
    }
    if let Some(issue) = &mut entry.plan_issue {
        issue.range = issue.range.take().map(|range| {
            let (start, end) = map((range.start, range.end));
            start..end
        });
    }
    for diagnostic in &mut entry.invalid {
        if let Some(ranges) = diagnostic.source_ranges.as_mut() {
            for range in ranges {
                let (start, end) = map((range.start, range.end));
                *range = start..end;
            }
        }
    }
    for property in &mut entry.properties {
        if let Some(ranges) = property.source_ranges.as_mut() {
            for range in ranges {
                let (start, end) = map((range.start, range.end));
                *range = start..end;
            }
        }
    }
    entry
}

fn map_template_record_note_line(
    line: &mut capture_language::record::RecordNoteLine,
    map: &impl Fn((usize, usize)) -> (usize, usize),
) {
    line.range = map(line.range);
    for child in &mut line.children {
        map_template_record_note_line(child, map);
    }
}

fn map_instance_rewrite(
    rewrite: &DiagnosticRewrite,
    instance: &capture_language::template::TemplateInstance,
    fields: &[capture_language::template::TemplateField],
) -> Option<DiagnosticRewrite> {
    // A rewrite must splice into the exact original draft, never synthetic
    // expansion coordinates. When the expanded rewrite spans more than one
    // field, it cannot be represented by one exact splice, so leave it
    // absent and retain the error.
    let touched = instance
        .source_map
        .iter()
        .filter(|segment| {
            ranges_overlap(
                Some((rewrite.start, rewrite.end)),
                Some((segment.expanded.start, segment.expanded.end)),
            )
        })
        .collect::<Vec<_>>();
    let field_ids = touched
        .iter()
        .filter_map(|segment| segment.field)
        .collect::<std::collections::BTreeSet<_>>();
    if field_ids.len() > 1 {
        return None;
    }
    // Reuse the range mapper for the single-field/literal case; it returns
    // an exact original-draft splice.
    let row_range = instance
        .source_map
        .first()
        .map(|segment| segment.source.start..segment.source.end)
        .unwrap_or(0..0);
    // Reconstruct a row_range from the instance's row range is not directly
    // available here; fall back to mapping via the same literal logic by
    // reusing the range mapper with a synthetic row range covering the
    // rewrite's original context is complex. Instead, map through the
    // instance's source map directly for the common single-segment case.
    // For simplicity, map start/end independently when they land in one
    // literal segment with equal lengths.
    let (mapped_start, mapped_end) = {
        let mut mapped: Vec<(usize, usize)> = Vec::new();
        for segment in &touched {
            let overlap_start = rewrite.start.max(segment.expanded.start);
            let overlap_end = rewrite.end.min(segment.expanded.end);
            if overlap_start >= overlap_end
                && !(rewrite.start == rewrite.end
                    && overlap_start == segment.expanded.start)
            {
                continue;
            }
            if segment.source.len() == segment.expanded.len() {
                let offset_start =
                    overlap_start.saturating_sub(segment.expanded.start);
                let offset_end =
                    overlap_end.saturating_sub(segment.expanded.start);
                mapped.push((
                    segment.source.start + offset_start,
                    segment.source.start + offset_end,
                ));
            } else {
                mapped.push((segment.source.start, segment.source.end));
            }
        }
        if mapped.is_empty() {
            return None;
        }
        let start = mapped.iter().map(|range| range.0).min()?;
        let end = mapped.iter().map(|range| range.1).max()?;
        // Multi-segment rewrites cannot be one exact splice.
        if touched.len() != 1 && field_ids.len() != 1 {
            // Allow contiguous literal spans that collapse to one range.
            // Otherwise leave absent.
            if mapped.len() != 1 {
                return None;
            }
        }
        (start, end)
    };
    let _ = (fields, row_range);
    Some(DiagnosticRewrite {
        start: mapped_start,
        end: mapped_end,
        replacement: rewrite.replacement.clone(),
        label: rewrite.label.clone(),
    })
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
    pomodoro_past: &Option<PomodoroPastSpec>,
    items: &[EditorItemParse],
    fallback_range: (usize, usize),
    source: &str,
) -> Option<Diagnostic> {
    use capture_language::SpanKind;

    if let Some(span) = spans.iter().find(|span| {
        span.kind == SpanKind::Directive
            && source.get(span.start..span.end).is_some_and(|token| {
                token.strip_prefix("%c").is_some_and(|rest| {
                    rest.is_empty()
                        || rest.bytes().all(|byte| byte.is_ascii_digit())
                })
            })
    }) {
        return Some(Diagnostic {
            severity: Severity::Error,
            code: "template_clipboard",
            message: "a template can't capture the clipboard: `%c` would repeat once per row".to_string(),
            range: Some((span.start, span.end)),
            rewrite: None,
        });
    }
    let is_session = |mode: EditorMode,
                      start: &Option<PomodoroStartSpec>,
                      close: &Option<PomodoroCloseSpec>,
                      past: &Option<PomodoroPastSpec>| {
        matches!(
            mode,
            EditorMode::PomodoroAdjust
                | EditorMode::PomodoroShift
                | EditorMode::PomodoroClose
                | EditorMode::PomodoroPast
                | EditorMode::PomodoroStart
        ) || matches!(mode, EditorMode::PomodoroTask | EditorMode::PomodoroLink)
            && (start.is_some() || close.is_some() || past.is_some())
    };
    let session =
        is_session(mode, pomodoro_start, pomodoro_close, pomodoro_past)
            || items.iter().any(|item| {
                is_session(
                    item.mode,
                    &item.pomodoro_start,
                    &item.pomodoro_close,
                    &item.pomodoro_past,
                )
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
                    | SpanKind::PomodoroPast
                    | SpanKind::PomodoroPastRepair
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
        rewrite: None,
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
        transfer: None,
        merge_in: None,
        needs: Vec::new(),
        ref_parent: None,
        ref_listen: false,
        sub_bullets: Vec::new(),
        sub_bullet_depths: Vec::new(),
        sub_bullet_task_ids: Vec::new(),
        sub_bullet_moves: Vec::new(),
        sub_bullet_move_choices: Vec::new(),
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_split: None,
        pomodoro_merge: None,
        pomodoro_close: None,
        pomodoro_past: None,
        dependencies: Vec::new(),
        blocked_tasks: Vec::new(),
        relations: Vec::new(),
        dependency_target: None,
        task_complete: None,
        list: None,
        header_range: None,
        template_row: None,
        targets: Vec::new(),
        record: None,
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
        route: global.route.clone(),
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

fn parse_items(
    items: &[EditorItemParse],
    transfers: &[capture_language::EditorTransferItem],
    records: &[super::capture_record::RecordParseEntry],
) -> Vec<CaptureParseItem> {
    // One `items[]` entry per list bullet, even when a lone header carries
    // a single bullet: the list object lives on the entry, not the batch.
    if items.len() <= 1 && items.first().is_none_or(|item| item.list.is_none())
    {
        return Vec::new();
    }
    items
        .iter()
        .map(|item| wire_item(item, transfers, records))
        .collect()
}

fn wire_item(
    item: &EditorItemParse,
    transfers: &[capture_language::EditorTransferItem],
    records: &[super::capture_record::RecordParseEntry],
) -> CaptureParseItem {
    let record = record_entry(records, item.index).map(parse_record);
    let mut needs = item.needs.clone();
    if record
        .as_ref()
        .is_some_and(|record| !record.missing.is_empty())
        && !needs.contains(&Need::RecordValue)
    {
        needs.push(Need::RecordValue);
    }
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
        transfer: transfers
            .iter()
            .find(|transfer| transfer.index == item.index)
            .map(|transfer| transfer.transfer.clone()),
        merge_in: item.merge_in.clone(),
        needs,
        list: item.list.clone(),
        header_range: item.list.as_ref().map(|list| SourceRange {
            start: list.header_start,
            end: list.header_end,
        }),
        ref_parent: ref_parent_for(
            item.mode,
            item.route.as_deref(),
            Some(item.has_local_destination),
        ),
        ref_listen: item.ref_listen,
        sub_bullets: sub_bullet_bodies(&item.sub_bullets),
        sub_bullet_depths: sub_bullet_depths(&item.sub_bullets),
        sub_bullet_task_ids: sub_bullet_task_ids(&item.sub_bullets),
        sub_bullet_moves: sub_bullet_moves(&item.sub_bullets),
        sub_bullet_move_choices: sub_bullet_move_choices(&item.sub_bullets),
        pomodoro_start: item.pomodoro_start.clone(),
        pomodoro_adjust: item.pomodoro_adjust.clone(),
        pomodoro_shift: item.pomodoro_shift.clone(),
        pomodoro_split: item.pomodoro_split.clone(),
        pomodoro_merge: item.pomodoro_merge.clone(),
        pomodoro_close: item.pomodoro_close.clone(),
        pomodoro_past: item.pomodoro_past.as_ref().map(past_wire),
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
        targets: parse_targets(&item.targets),
        record,
    }
}

fn record_entry(
    entries: &[super::capture_record::RecordParseEntry],
    index: usize,
) -> Option<&super::capture_record::RecordParseEntry> {
    entries.iter().find(|entry| entry.index == index)
}

fn parse_record(
    entry: &super::capture_record::RecordParseEntry,
) -> ParseRecord {
    ParseRecord {
        action: "create",
        record_type: entry.record_type.id.clone(),
        label: entry.record_type.label.clone(),
        emoji: entry.record_type.emoji.clone(),
        dir: entry.record_type.dir.clone(),
        path: entry.path.as_ref().map(|path| path.display().to_string()),
        stem: entry.stem.clone(),
        stem_source: entry.stem_source.map(|source| match source {
            super::records::RecordStemSource::Title => "title",
            super::records::RecordStemSource::Explicit => "explicit",
        }),
        title: entry.intent.title.clone(),
        missing: entry.missing.clone(),
        notes: entry
            .intent
            .notes
            .iter()
            .filter(|group| group.section.is_none())
            .map(|group| group.lines.len())
            .sum(),
        fields: entry.properties.iter().map(parse_record_field).collect(),
    }
}

fn parse_record_field(
    field: &super::records::PlannedRecordProperty,
) -> ParseRecordField {
    use super::records::{FieldKind, RecordPropertySource};

    let ranges = field
        .source_ranges
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|range| SourceRange {
            start: range.start,
            end: range.end,
        })
        .collect::<Vec<_>>();
    let choices = match &field.kind {
        FieldKind::Choices(choices) => choices
            .iter()
            .map(|choice| ParseRecordChoice {
                value: choice.value.clone(),
                label: choice.label.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    ParseRecordField {
        key: field.key.clone(),
        label: field.label.clone(),
        kind: match &field.kind {
            FieldKind::Text => "text",
            FieldKind::Choices(_) => "choices",
            FieldKind::Date => "date",
            FieldKind::Link => "link",
            FieldKind::Url => "url",
            FieldKind::Number => "number",
        },
        required: field.required,
        list: field.list,
        state: field.state.as_str(),
        value: field
            .value
            .as_ref()
            .map(super::capture_record::record_value_json),
        display: field.display.clone(),
        source: field.source.map(|source| match source {
            RecordPropertySource::Typed => "typed",
            RecordPropertySource::Default => "default",
        }),
        range: (ranges.len() == 1).then(|| ranges[0]),
        value_ranges: if ranges.len() > 1 { ranges } else { Vec::new() },
        choices,
    }
}

fn record_parse_diagnostics(
    input: &str,
    entries: &[super::capture_record::RecordParseEntry],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for entry in entries {
        if let Some(issue) = &entry.plan_issue
            && !(issue.code == "record_title"
                && entry.intent.title.trim().is_empty())
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: issue.code,
                message: issue.message.clone(),
                range: issue
                    .range
                    .as_ref()
                    .map(|range| (range.start, range.end)),
                rewrite: None,
            });
        }
        for issue in &entry.invalid {
            let code = issue.diagnostic_code();
            let mut message = issue.message.clone();
            if !issue.suggestions.is_empty() {
                message.push_str("; did you mean ");
                message.push_str(
                    &issue
                        .suggestions
                        .iter()
                        .map(|value| format!("`{value}`"))
                        .collect::<Vec<_>>()
                        .join(" or "),
                );
                message.push('?');
            }
            let range = issue
                .field
                .as_deref()
                .and_then(|key| {
                    if code == "record_unknown_field" {
                        entry
                            .intent
                            .fields
                            .iter()
                            .find(|field| field.key == key)
                            .map(|field| field.key_range)
                    } else {
                        issue
                            .source_ranges
                            .as_ref()
                            .and_then(|ranges| ranges.first())
                            .map(|range| (range.start, range.end))
                    }
                })
                .or_else(|| {
                    issue
                        .source_ranges
                        .as_ref()
                        .and_then(|ranges| ranges.first())
                        .map(|range| (range.start, range.end))
                });
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code,
                message,
                range: range.filter(|(start, end)| {
                    *start <= *end && *end <= input.len()
                }),
                rewrite: None,
            });
        }
        diagnostics.extend(single_colon_diagnostics(entry));
    }
    diagnostics
}

fn single_colon_diagnostics(
    entry: &super::capture_record::RecordParseEntry,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for group in &entry.intent.notes {
        if group.section.is_some() {
            continue;
        }
        for line in &group.lines {
            for field in &entry.record_type.fields {
                let Some(rest) = line.text.strip_prefix(&field.key) else {
                    continue;
                };
                if !rest.starts_with(':') || rest.starts_with("::") {
                    continue;
                }
                let end = line.range.0 + field.key.len() + 1;
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "record_field_single_colon",
                    message: format!(
                        "`{}:` stays a note; use `{}:: value` to set the {} field",
                        field.key, field.key, entry.record_type.label,
                    ),
                    range: Some((line.range.0, end.min(line.range.1))),
                    rewrite: None,
                });
                break;
            }
        }
    }
    diagnostics
}

fn collection_route_diagnostics(
    input: &str,
    items: &[EditorItemParse],
    policy: Option<&capture_language::RecordRoutePolicy>,
) -> Vec<Diagnostic> {
    let Some(policy) = policy else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if item.mode != EditorMode::Task
                || !item.has_local_destination
                || item.section.is_some()
                || item.block_id.is_some()
                || item.list.is_some()
            {
                return None;
            }
            let route = item.route.as_deref()?;
            let record_type = policy.configured_dir(route)?;
            let span = item
                .spans
                .iter()
                .find(|span| span.kind == capture_language::SpanKind::Route)?;
            let token = input.get(span.start..span.end)?;
            if token != format!("@{route}") {
                return None;
            }
            let label = policy
                .records_config()?
                .record_type(record_type)?
                .label
                .as_str();
            Some(Diagnostic {
                severity: Severity::Warning,
                code: "record_collection_route",
                message: format!(
                    "{}.md is the {label} collection — `@{route}/` creates a record note",
                    route,
                ),
                range: Some((span.start, span.end)),
                rewrite: Some(DiagnosticRewrite {
                    start: span.start,
                    end: span.end,
                    replacement: format!("@{route}/"),
                    label: format!("Use `@{route}/` to create a {label} note"),
                }),
            })
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

fn sub_bullet_moves(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<Option<MoveBullet>> {
    sub_bullets
        .iter()
        .map(|item| item.move_task.clone())
        .collect()
}

fn sub_bullet_move_choices(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<Option<MoveChoiceBullet>> {
    sub_bullets
        .iter()
        .map(|item| item.move_choice.clone())
        .collect()
}

fn all_task_ids_none(ids: &[Option<ProjectTaskId>]) -> bool {
    ids.iter().all(Option::is_none)
}

fn all_moves_none(moves: &[Option<MoveBullet>]) -> bool {
    moves.iter().all(Option::is_none)
}

fn all_choices_none(choices: &[Option<MoveChoiceBullet>]) -> bool {
    choices.iter().all(Option::is_none)
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
        if merged.last().is_some_and(|previous: &Span| {
            if previous.start == span.start && previous.end == span.end {
                // Same bytes: keep both when they say different things
                // (a lone move sigil carries its marker plus the picker
                // placeholder); drop an exact duplicate.
                previous.kind == span.kind
            } else {
                previous.end > span.start
            }
        }) {
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
        let summary = global
            .route
            .as_deref()
            .unwrap_or("(incomplete)")
            .to_string();
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
        // A whole-item named start (`=name`, `=<X>name`, or
        // `=<X>#name` when needed) folds its name into the start line;
        // a `@<route>:<block-id>=<X><name>`
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
    if let Some(split) = result.pomodoro_split.as_ref() {
        print_field(styler, "split", &format_pomodoro_split(split));
    }
    if let Some(merge) = result.pomodoro_merge.as_ref() {
        print_field(styler, "merge", &format_pomodoro_merge(merge));
    }
    if let Some(close) = result.pomodoro_close.as_ref() {
        print_field(styler, "close", &format_pomodoro_close(close));
    }
    if let Some(past) = result.pomodoro_past.as_ref() {
        print_field(styler, "past", &format_pomodoro_past(past));
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
            if let Some(move_bullet) = result
                .sub_bullet_moves
                .get(index)
                .and_then(|entry| entry.as_ref())
            {
                let id = move_bullet.block_id.as_deref().unwrap_or_default();
                if move_bullet.sigil == ':' {
                    println!("    {indentation}- move & link :{id}");
                } else {
                    println!("    {indentation}- move ^{id}");
                }
                continue;
            }
            if result
                .sub_bullet_move_choices
                .get(index)
                .and_then(|entry| entry.as_ref())
                .is_some()
            {
                println!("    {indentation}- move choice ^^");
                continue;
            }
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
/// token (`=` plus `<X>`, plus the shortest valid name spelling and
/// `~<K>` for a drop list) plus its resolved 5-minute duration and offset
/// units.
fn format_pomodoro_start(
    start: &PomodoroStartSpec,
    section: Option<&str>,
) -> String {
    let sigil = "=";
    let mut token = match section {
        Some(name) if capture_language::can_spell_bare(name) => {
            format!("{sigil}{}{name}", start.raw)
        }
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
    let offset = if start.later_units > 0 {
        format!("+{}", start.later_units)
    } else if start.offset_units > 0 {
        format!("-{}", start.offset_units)
    } else {
        "0".to_string()
    };
    let duration = start.duration_units.saturating_mul(5);
    if start.drop.is_empty() {
        format!("{token} ({duration}m, offset {offset}u)")
    } else {
        format!(
            "{token} ({duration}m, offset {offset}u · drop {})",
            join_numbers(&start.drop)
        )
    }
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

fn format_pomodoro_merge(merge: &PomodoroMergeSpec) -> String {
    let dests = merge
        .dests
        .iter()
        .map(|dest| {
            if dest.literal {
                format!("#\"{}\"", dest.text)
            } else {
                format!("#{}", dest.text)
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if let Some(src) = merge.src.as_deref() {
        format!("=*{src}{dests}")
    } else if dests.is_empty() {
        "=*".to_string()
    } else {
        format!("=*{dests}")
    }
}

fn format_pomodoro_split(split: &PomodoroSplitSpec) -> String {
    if split.selectors.is_empty() {
        if let Some(base) = split.base.as_deref() {
            return format!("=/{} (all merged-in types)", base);
        }
        return "=/ (all merged-in types)".to_string();
    }
    let selectors = split
        .selectors
        .iter()
        .map(|selector| {
            if selector.literal {
                format!("#\"{}\"", selector.text)
            } else {
                format!("#{}", selector.text)
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if let Some(base) = split.base.as_deref() {
        format!("=/{}{}", base, selectors)
    } else {
        format!("=/{}", selectors)
    }
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
/// any numbered list without a wildcard ends with `defer the rest`, and `x0` reads
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
                let head = if entry.all {
                    format!("* {:?}", entry.text)
                } else {
                    match entry.index {
                        Some(index) => format!("{index} {:?}", entry.text),
                        None => format!("{:?}", entry.text),
                    }
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
    let has_work_selection =
        close.is_explicit() || close.park_all || close.complete_all;
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
            // A modifier list after `x0` makes the empty `<N>` redundant.
            if close.park.is_empty()
                && close.complete.is_empty()
                && close.drop.is_empty()
                && !close.park_all
                && !close.complete_all
            {
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

/// Render a validated `x=` past for human output: the trimmed past token
/// exactly as typed plus the text-only summary. The parenthetical lists the
/// canonical name (when typed), minutes, stack position (only when the run
/// has more than one member), `repair` (when typed), and the close summary
/// (when the close carries work). For example `x=fix3!=*3` with a second
/// stack member reads `past x=fix3!=*3 (FIX · 15m · 2 of 2 · repair ·
/// close: parked 3 · defer the rest)`.
fn format_pomodoro_past(past: &PastWire) -> String {
    let base = past.raw.clone();
    let mut parts = Vec::new();
    if let Some(segment) = past.segment.as_ref() {
        parts.push(format!("segment {}", segment.raw));
    }
    if let Some(name) = past.name.as_deref() {
        parts.push(name.to_string());
    }
    parts.push(format!("{}m", past.minutes));
    if past.stack.size > 1 {
        parts.push(format!("{} of {}", past.stack.index, past.stack.size));
    }
    if past.repair {
        parts.push("repair".to_string());
    }
    if let Some(summary) = format_past_close_summary(&past.close) {
        parts.push(format!("close: {summary}"));
    }
    format!("{base} ({})", parts.join(" · "))
}

/// Close summary for a past human line, without the close `raw`: `None`
/// for a plain close with no logs, otherwise the work summary (`parked`,
/// `complete`, `drop`, `log`, `defer the rest`, `in progress none`).
fn format_past_close_summary(close: &PomodoroCloseSpec) -> Option<String> {
    let log_part = if close.log.is_empty() {
        None
    } else {
        let entries = close
            .log
            .iter()
            .map(|entry| {
                let head = if entry.all {
                    format!("* {:?}", entry.text)
                } else {
                    match entry.index {
                        Some(index) => format!("{index} {:?}", entry.text),
                        None => format!("{:?}", entry.text),
                    }
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
    let has_work_selection =
        close.is_explicit() || close.park_all || close.complete_all;
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
            return None;
        }
        return Some(parts.join(" · "));
    }
    let mut parts = Vec::new();
    if let Some(in_progress) = close.in_progress.as_ref() {
        if in_progress.is_empty() {
            if close.park.is_empty()
                && close.complete.is_empty()
                && close.drop.is_empty()
                && !close.park_all
                && !close.complete_all
            {
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
    Some(parts.join(" · "))
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

    fn record_parse_config(vault: &std::path::Path) -> std::path::PathBuf {
        let path = vault.join("config.yml");
        std::fs::write(
            &path,
            r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: "🍽️"
    dated_notes: true
    fields:
      status:
        required: true
        values:
          to_try: "🔖 To try"
          liked: "💚 Liked"
      category:
        required: true
      location:
        required: true
      companions:
        values: link
        list: true
        default: ["[[kelly]]"]
      visited:
        values: date
"#,
        )
        .expect("write record test config");
        path
    }

    fn parse_with_records(
        input: &str,
        vault: &std::path::Path,
    ) -> CaptureParseResult {
        let config = record_parse_config(vault);
        let policy = super::super::capture_record::record_policy_for_draft(
            vault,
            Some(&config),
            input,
        );
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: Some(&policy),
        };
        CaptureParseResult::new(input.to_string(), &options)
    }

    fn record_json(input: &str, vault: &std::path::Path) -> serde_json::Value {
        serde_json::to_value(parse_with_records(input, vault)).unwrap()
    }

    #[test]
    fn record_parse_projects_engine_fields_defaults_and_source_ranges() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let input = concat!(
            "Ninja Sushi @eat/\n",
            "- status:: liked\n",
            "- category:: Sushi\n",
            "- location:: Clark\n",
            "- visited:: today\n",
            "- Great lunch.\n",
        );
        let value = record_json(input, vault.path());
        let record = &value["record"];
        assert_eq!(record["action"], "create");
        assert_eq!(record["type"], "restaurant");
        assert_eq!(record["label"], "Restaurant");
        assert_eq!(record["emoji"], "🍽️");
        assert_eq!(record["dir"], "eat");
        assert_eq!(record["path"], "eat/ninja_sushi.md");
        assert_eq!(record["stem"], "ninja_sushi");
        assert_eq!(record["stem_source"], "title");
        assert_eq!(record["title"], "Ninja Sushi");
        assert_eq!(record["missing"], serde_json::json!([]));
        assert_eq!(record["notes"], 1);

        let fields = record["fields"].as_array().unwrap();
        let status = fields
            .iter()
            .find(|field| field["key"] == "status")
            .unwrap();
        assert_eq!(status["kind"], "choices");
        assert_eq!(status["state"], "set");
        assert_eq!(status["value"], "liked");
        assert_eq!(status["display"], "💚 Liked");
        assert_eq!(status["source"], "typed");
        assert_eq!(status["choices"][1]["value"], "liked");
        assert!(status.get("value_ranges").is_none());
        let status_range = &status["range"];
        assert_eq!(
            &input[status_range["start"].as_u64().unwrap() as usize
                ..status_range["end"].as_u64().unwrap() as usize],
            "liked"
        );

        let companions = fields
            .iter()
            .find(|field| field["key"] == "companions")
            .unwrap();
        assert_eq!(companions["state"], "default");
        assert_eq!(companions["source"], "default");
        assert_eq!(companions["value"][0], "[[kelly]]");
    }

    #[test]
    fn record_parse_retains_empty_missing_and_invalid_field_states() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let missing = record_json(
            "Ninja Sushi @eat/\n- status:: liked\n- category:: Sushi\n",
            vault.path(),
        );
        assert_eq!(
            missing["record"]["missing"],
            serde_json::json!(["location"])
        );
        assert_eq!(missing["needs"][0], "record_value");
        assert_eq!(missing["record"]["fields"][2]["state"], "missing");

        let none = record_json(
            "Ninja Sushi @eat/\n- status:: liked\n- category:: Sushi\n- location:: Clark\n- companions::\n",
            vault.path(),
        );
        let companions = none["record"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "companions")
            .unwrap();
        assert_eq!(companions["state"], "none");
        assert!(companions["value"].is_null());
        assert_eq!(companions["range"]["start"], companions["range"]["end"]);

        let invalid = record_json(
            "Ninja Sushi @eat/\n- status:: loved\n- category:: Sushi\n- location:: Clark\n",
            vault.path(),
        );
        let status = invalid["record"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "status")
            .unwrap();
        assert_eq!(status["state"], "invalid");
        assert!(invalid["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "record_invalid_value"));
    }

    #[test]
    fn record_parse_keeps_schema_for_incomplete_titles_and_unknown_fields() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let incomplete = record_json("@eat/", vault.path());
        assert_eq!(incomplete["mode"], "record_note");
        assert!(incomplete["needs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|need| need == "record_title"));
        assert!(incomplete["record"]["path"].is_null());
        assert_eq!(incomplete["record"]["type"], "restaurant");
        assert_eq!(incomplete["record"]["fields"].as_array().unwrap().len(), 5);

        let unknown = record_json(
            "Ninja Sushi @eat/\n- status:: liked\n- category:: Sushi\n- location:: Clark\n- occasion:: birthday\n",
            vault.path(),
        );
        assert!(unknown["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "record_unknown_field"));
    }

    #[test]
    fn record_parse_reports_invalid_records_configuration() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let config = vault.path().join("config.yml");
        std::fs::write(&config, "records: [not, a, mapping]\n")
            .expect("write malformed record config");
        let input = "Ninja Sushi @eat/";
        let policy = super::super::capture_record::record_policy_for_draft(
            vault.path(),
            Some(&config),
            input,
        );
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: Some(&policy),
        };
        let parsed = CaptureParseResult::new(input.to_string(), &options);
        let value = serde_json::to_value(parsed).unwrap();
        assert!(value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "records_config_invalid"));
    }

    #[test]
    fn record_parse_keeps_repeated_field_ranges_disjoint_from_notes() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let input = concat!(
            "Ninja Sushi @eat/\n",
            "- status:: liked\n",
            "- category:: Sushi\n",
            "- location:: Clark\n",
            "- companions:: [[kelly]]\n",
            "- A note between values.\n",
            "- companions:: [[sam]]\n",
        );
        let value = record_json(input, vault.path());
        let companions = value["record"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "companions")
            .unwrap();
        assert!(companions["range"].is_null());
        let ranges = companions["value_ranges"].as_array().unwrap();
        assert_eq!(ranges.len(), 2);
        for (range, expected) in ranges.iter().zip(["[[kelly]]", "[[sam]]"]) {
            assert_eq!(
                &input[range["start"].as_u64().unwrap() as usize
                    ..range["end"].as_u64().unwrap() as usize],
                expected
            );
        }
        assert!(input[ranges[0]["end"].as_u64().unwrap() as usize
            ..ranges[1]["start"].as_u64().unwrap() as usize]
            .contains("A note between values."));
        assert_eq!(value["record"]["notes"], 1);
    }

    #[test]
    fn record_parse_maps_explicit_stems_single_colon_and_collection_fix() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let explicit = record_json(
            "@eat/^nagi-shop Ninja Sushi\n- location:: Clark\n",
            vault.path(),
        );
        assert_eq!(explicit["record"]["path"], "eat/nagi-shop.md");
        assert_eq!(explicit["record"]["stem_source"], "explicit");

        let hint = record_json(
            "Ninja Sushi @eat/\n- status:: liked\n- category:: Sushi\n- location: upstairs\n",
            vault.path(),
        );
        let single_colon = hint["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| {
                diagnostic["code"] == "record_field_single_colon"
            })
            .expect("single-colon hint");
        assert_eq!(single_colon["severity"], "warning");

        let collection = record_json("Blaze @eat", vault.path());
        let warning = collection["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "record_collection_route")
            .expect("collection near-miss warning");
        let start = warning["rewrite"]["start"].as_u64().unwrap() as usize;
        let end = warning["rewrite"]["end"].as_u64().unwrap() as usize;
        let fixed = format!(
            "{}{}{}",
            "Blaze @eat"[..start].to_string(),
            warning["rewrite"]["replacement"].as_str().unwrap(),
            "Blaze @eat"[end..].to_string(),
        );
        assert_eq!(record_json(&fixed, vault.path())["mode"], "record_note");

        for literal in ["Blaze \\@eat", "Blaze @eat+task-id", "@eat#Dinner"] {
            let value = record_json(literal, vault.path());
            assert!(value["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|diagnostic| diagnostic["code"]
                    != "record_collection_route"));
        }
    }

    #[test]
    fn record_parse_reports_per_item_and_same_draft_collisions() {
        let vault = tempfile::tempdir().expect("scratch vault");
        let input = concat!(
            "Ninja Sushi @eat/\n",
            "- status:: liked\n- category:: Sushi\n- location:: Clark\n\n",
            "Ninja Sushi @eat/\n",
            "- status:: liked\n- category:: Sushi\n- location:: Clark\n",
        );
        let value = record_json(input, vault.path());
        let items = value["items"].as_array().expect("item projections");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["record"]["path"], "eat/ninja_sushi.md");
        assert_eq!(items[1]["record"]["path"], "eat/ninja_sushi.md");
        assert!(value["diagnostics"].as_array().unwrap().iter().any(
            |diagnostic| {
                diagnostic["code"] == "record_exists"
                    && diagnostic["message"]
                        .as_str()
                        .unwrap()
                        .contains("@eat/ninja_sushi")
            }
        ));
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
        let themes = task_type_themes("Fix it #deep-work", &catalog);
        assert_eq!(themes.as_array().unwrap().len(), 1);
        assert_eq!(themes[0]["key"], "deep work");
        assert_eq!(themes[0]["name"], "DEEP WORK");
        assert!(themes[0].get("known").is_none());
        assert_eq!(
            &"Fix it #deep-work"[themes[0]["start"].as_u64().unwrap() as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "deep-work"
        );
        // A literal resolves by key equality with quotes excluded.
        let themes = task_type_themes("Fix it #\"DEEP WORK\"", &catalog);
        assert_eq!(themes[0]["key"], "deep work");
        assert_eq!(
            &"Fix it #\"DEEP WORK\""[themes[0]["start"].as_u64().unwrap()
                as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "DEEP WORK"
        );
        // Curly quotes keep char-boundary name ranges.
        let themes = task_type_themes("Fix it #“FIX”", &catalog);
        assert_eq!(themes[0]["key"], "fix");
        assert_eq!(
            &"Fix it #“FIX”"[themes[0]["start"].as_u64().unwrap() as usize
                ..themes[0]["end"].as_u64().unwrap() as usize],
            "FIX"
        );
        // An unknown name carries no fabricated resolution.
        let themes = task_type_themes("Fix it #nope", &catalog);
        assert_eq!(themes.as_array().unwrap().len(), 1);
        assert_eq!(themes[0]["key"], "nope");
        assert_eq!(themes[0]["name"], "NOPE");
        assert_eq!(themes[0]["known"], false);
        // Quoted composites are invalid markers and never fabricate a ref.
        for raw in ["Fix it #\"a+b\"", "Fix it #“a+b”"] {
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
            CaptureParseResult::new("Fix it #\"".to_string(), &options);
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
        assert_eq!(value["grammar"], 20);
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
    fn json_reports_targets_for_public_runs() {
        let raw = "@work+a @dev+b note";
        // The production result builder activates target runs by default.
        let default = json(raw);
        let default_targets = default["targets"].as_array().expect("targets");
        assert_eq!(default_targets.len(), 2);
        assert_eq!(default["body"], "note");

        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: None,
        };
        let result = CaptureParseResult::new(raw.to_string(), &options);
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value, default);
        let targets = value["targets"].as_array().expect("targets");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0]["marker"], "@work+a");
        assert_eq!(targets[1]["marker"], "@dev+b");
        assert_eq!(targets[0]["mode"], "sub_bullet");
        assert_eq!(targets[0]["route"], "work");
        assert_eq!(targets[0]["block_id"], "a");
        assert!(targets[0].get("section").is_none());
        let first_at = raw.find("@work+a").unwrap();
        let second_at = raw.find("@dev+b").unwrap();
        assert_eq!(targets[0]["start"], first_at);
        assert_eq!(targets[0]["end"], first_at + "@work+a".len());
        assert_eq!(targets[1]["start"], second_at);
        assert_eq!(value["body"], "note");
        assert_eq!(value["route"], "work");
        // Never expands items[] for a single-item draft.
        assert!(value.get("items").is_none(), "{value}");
    }

    #[test]
    fn lone_route_reports_note_pull_mode() {
        // NP27: `bob capture-parse -f json -- @sase` reports mode
        // `note_pull` with no needs, the route, and the ordinary list
        // summary (`kind: "tasks"`, `count: 0`) under grammar 14. A lone
        // header keeps its task-list metadata so clients can still offer
        // "start a list" from it.
        let value = json("@sase");
        assert_eq!(value["ok"], true);
        assert_eq!(value["grammar"], 20);
        assert_eq!(value["mode"], "note_pull");
        assert_eq!(value["route"], "sase");
        assert!(value["section"].is_null());
        assert!(value["block_id"].is_null());
        assert_eq!(value["needs"].as_array().expect("needs").len(), 0);
        assert_eq!(value["diagnostics"].as_array().expect("array").len(), 0);
        assert_eq!(value["list"]["kind"], "tasks");
        assert_eq!(value["list"]["count"], 0);
        assert_eq!(value["list"]["header_line"], 1);
        assert_eq!(
            value["items"].as_array().expect("items").len(),
            1,
            "{value}"
        );
        assert_eq!(value["items"][0]["mode"], "note_pull", "{value}");
        assert_eq!(value["items"][0]["list"]["count"], 0, "{value}");
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

        let value = json("body $2 s:1 %c @groceries");
        assert_eq!(value["body"], "body");
        assert_eq!(
            value["spans"]
                .as_array()
                .expect("array")
                .iter()
                .map(|span| span["kind"].as_str().expect("kind"))
                .collect::<Vec<_>>(),
            vec!["priority", "schedule", "directive", "route"]
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
    fn json_reports_glued_start_suffix_names() {
        let value = json("Do work @sase:outline=3abc");
        assert_eq!(value["pomodoro_start"]["raw"], "3");
        assert_eq!(value["section"], "abc");
        assert_eq!(value["diagnostics"], serde_json::json!([]));

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
        let value = json("=bugs");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["mode"], "pomodoro_start");
        assert_eq!(value["body"], "=bugs");
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
                { "start": 1, "end": 5, "kind": "pomodoro_name" },
            ])
        );
        assert_eq!(value["diagnostics"], serde_json::json!([]));
        assert!(value.get("items").is_none(), "{value}");

        let counted = json("=3bugs");
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
                { "start": 2, "end": 6, "kind": "pomodoro_name" },
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
        assert_eq!(order["diagnostics"][0]["range"], serde_json::json!([1, 2]));

        let multiword = json("=deep work");
        assert_eq!(
            multiword["diagnostics"][0]["code"],
            "invalid_pomodoro_start"
        );
        assert_eq!(
            multiword["diagnostics"][0]["range"],
            serde_json::json!([6, 10])
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

        let chain = json("x =bugs");
        assert_eq!(chain["mode"], "pomodoro_close");
        assert_eq!(chain["items"].as_array().expect("items").len(), 2);
        assert_eq!(chain["items"][0]["mode"], "pomodoro_close");
        assert_eq!(chain["items"][1]["mode"], "pomodoro_start");
        assert_eq!(chain["items"][1]["section"], "bugs");
        assert_eq!(chain["items"][1]["body"], "=bugs");
        assert_eq!(
            chain["items"][1]["range"],
            serde_json::json!({ "start": 2, "end": 7 })
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
            let rewrite = match raw {
                "==3#bugs" => "=3bugs",
                "==#bugs=3" => "=bugs3",
                _ => raw.strip_prefix('=').expect("doubled token"),
            };
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
        assert_eq!(value["global_destination"]["route"], "foo");
        assert_eq!(value["global_destination"]["line"], 1);
        assert!(value["global_destination"].get("mode").is_none());
        assert!(value["global_destination"].get("block_id").is_none());
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
    fn json_reports_global_route_with_no_mode_or_block_id_fields() {
        let raw = "@@foo\nNote one\n\nIndependent @bar";
        let value = json(raw);
        assert_eq!(value["global_destination"]["route"], "foo");
        assert!(value["global_destination"].get("mode").is_none());
        assert!(value["global_destination"].get("block_id").is_none());
        assert_eq!(value["items"][0]["mode"], "task");
        assert_eq!(value["items"][0]["route"], "foo");
        assert_eq!(value["items"][1]["mode"], "task");
        assert_eq!(value["items"][1]["route"], "bar");
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
                    all: false,
                    origin: Default::default(),
                },
                CloseLogEntry {
                    index: Some(2),
                    text: "sketched the parser".to_string(),
                    details: vec!["a".to_string(), "b".to_string()],
                    all: false,
                    origin: Default::default(),
                },
                CloseLogEntry {
                    index: Some(1),
                    text: "opened the PR".to_string(),
                    details: Vec::new(),
                    all: false,
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
        let star = PomodoroCloseSpec {
            raw: "x".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: false,
            drop: Vec::new(),
            log: vec![
                CloseLogEntry {
                    index: None,
                    text: "wired the lexer".to_string(),
                    details: Vec::new(),
                    all: false,
                    origin: Default::default(),
                },
                CloseLogEntry {
                    index: None,
                    text: "code review".to_string(),
                    details: vec!["cleaned up".to_string()],
                    all: true,
                    origin: crate::native::capture_language::CloseLogOrigin::StarBullet {
                        position: 2,
                    },
                },
            ],
        };
        assert_eq!(
            format_pomodoro_close(&star),
            "x (log \"wired the lexer\", * \"code review\" (+1 detail))"
        );
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

        let complete_one = PomodoroCloseSpec {
            raw: "x!2".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: vec![2],
            complete_all: false,
            drop: Vec::new(),
            log: Vec::new(),
        };
        assert_eq!(
            format_pomodoro_close(&complete_one),
            "x!2 (complete 2 · defer the rest)"
        );

        let complete_one_with_zero = PomodoroCloseSpec {
            raw: "x0!2".to_string(),
            in_progress: Some(Vec::new()),
            ..complete_one.clone()
        };
        assert_eq!(
            format_pomodoro_close(&complete_one_with_zero),
            "x0!2 (complete 2 · defer the rest)"
        );

        let drop_one = PomodoroCloseSpec {
            raw: "x~2".to_string(),
            in_progress: None,
            park: Vec::new(),
            park_all: false,
            complete: Vec::new(),
            complete_all: false,
            drop: vec![2],
            log: Vec::new(),
        };
        assert_eq!(
            format_pomodoro_close(&drop_one),
            "x~2 (drop 2 · defer the rest)"
        );
    }

    #[test]
    fn json_reports_a_declaration_only_draft_as_a_diagnostic() {
        let value = json("@@foo");
        assert_eq!(value["ok"], true);
        assert_eq!(value["global_destination"]["route"], "foo");
        assert_eq!(value["diagnostics"][0]["code"], "missing_capture_item");
        assert!(value.get("items").is_none(), "{value}");
    }

    #[test]
    fn json_reports_merge_in_wire_shape_and_grammar_20() {
        let value = json("New task < @a+x @b+y");
        assert_eq!(value["ok"], true);
        assert_eq!(value["grammar"], 20);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["mode"], "task");
        let merge_in = value.get("merge_in").expect("merge_in");
        assert_eq!(merge_in["sources"].as_array().expect("sources").len(), 2);
        assert_eq!(merge_in["sources"][0]["marker"], "@a+x");
        assert_eq!(merge_in["sources"][0]["route"], "a");
        assert_eq!(merge_in["sources"][0]["block_id"], "x");
        let kinds = value["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .map(|span| span["kind"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"merge_in_arrow"));
        assert!(kinds.contains(&"merge_in_source"));
        assert!(!kinds.contains(&"transfer_source"));
        assert!(value.get("transfer").is_none());
        // Ordinary tasks omit the additive key.
        let plain = json("Just a task @work");
        assert!(plain.get("merge_in").is_none());
        assert!(plain.get("transfer").is_none());
    }

    #[test]
    fn json_reports_merge_in_pending_need_without_diagnostic() {
        let value = json("New task < @a");
        assert_eq!(value["ok"], true);
        assert!(value["needs"]
            .as_array()
            .expect("needs")
            .iter()
            .any(|need| need == "merge_in_source"));
        assert!(value["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .any(|span| span["kind"] == "interactive_placeholder"));
        assert_eq!(
            value["diagnostics"].as_array().expect("diagnostics").len(),
            0
        );
        assert!(value.get("merge_in").is_some());
    }

    #[test]
    fn json_reports_merge_in_rewrite_and_reparses() {
        let value = json("New outcome < @foo+f s:3");
        assert_eq!(value["ok"], true);
        let diagnostic = value["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .find(|diagnostic| diagnostic["code"] == "merge_in_extra_text")
            .expect("extra-text diagnostic");
        let rewrite = diagnostic.get("rewrite").expect("rewrite");
        assert_eq!(rewrite["label"], "Move before <");
        let replacement = rewrite["replacement"].as_str().expect("replacement");
        let reparsed = json(replacement);
        assert_eq!(
            reparsed["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .len(),
            0
        );
        assert!(reparsed.get("merge_in").is_some());
    }
}
