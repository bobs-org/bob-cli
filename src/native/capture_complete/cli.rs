use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    iter,
    path::PathBuf,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};

use super::COMMAND_NAME;
use super::{
    model::{CompleteError, OutputFormat},
    render::{print_error, print_success},
};
use crate::native::capture_flag_guard;
use crate::native::env as bob_env;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    if let Some(hit) = capture_flag_guard::find_unknown_long_flag(
        &args,
        &capture_flag_guard::capture_complete_spec(),
    ) {
        let message =
            capture_flag_guard::unexpected_argument_message(&hit.flag);
        if hit.format_json {
            return print_error(
                &CompleteError::usage(message),
                OutputFormat::Json,
            );
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
    let all_tasks = matches.get_flag("all-tasks");
    let closed_tasks = matches.get_flag("closed-tasks");
    let next_missing_field = matches.get_flag("next-missing-field");
    let record_field = matches
        .get_one::<String>("record-field")
        .map(String::as_str);
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

    match super::engine::build_result_for(&super::engine::CompleteRequest {
        bob_dir: &bob_dir,
        raw_text: &raw_text,
        cursor,
        all_tasks,
        closed_tasks,
        next_missing_field,
        record_field,
    }) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(&error, output_format),
    }
}

pub(super) fn print_clap_error(error: clap::Error) -> i32 {
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
bullet marker itself, an orphaned nested line, or a cursor on a middle token that is neither a member of the line's `@route+id` target run nor the `@`-fragment extending that run at its inner edge all return a \
successful empty result rather than an error. Every member of a target run completes through its own route and block ID, and the `@`-fragment right after a leading run or right before a trailing run extends that run once completed.\n\n\
Route completion covers a bare '@', a still-typing '@fragment', and the \
missing route portion of '@^...', '@+...', '@:...', and '@#...', plus the route \
component of a '@@' or '@@fragment' declaration anywhere in the draft. Replacement \
ranges for that declaration exclude both '@' sigils. Parent-task completion \
covers '@route+fragment', including -a/--all-tasks missing-ID candidates. An inherited global route also \
becomes the current note for same-note wikilink heading/block completion unless \
that item overrides it. Route completion is backed by the same \
scan as `bob capture-targets`. Section completion covers '@route#prefix', \
'@route#\"partial', and a list-header '@route#prefix' even after bullets \
are typed below it, backed by the same scan as `bob capture-sections`. Each \
row's replacement is a selector that resolves back to that heading: the raw \
title when it is one unquoted token, else the hyphen slug, else a quoted \
'\"Title\"'. Drawer completion \
covers '@route+id#prefix' and a bare '@route+id#' once the item has body text, backed by the same \
scanner as `bob capture-drawers`; the replacement range and text cover the \
`#` and the name (`#future-work`, or a quoted `#\"TITLE\"` when the slug does \
not resolve back to the same drawer). Bullet drawer completion covers the '#' that \
starts a first-level bullet (`- #`, `- #fut`) where its bullets land directly under a \
task (a new task, a new project note, or a note-less '@route+id'): every row's \
replacement is the bare ALL-CAPS title (`FUTURE WORK`), with a nested `  - ` bullet \
opened after it when nothing nested follows, and a bullet-start '#' that cannot name \
a drawer returns a successful empty result. A first-level bullet's \
leading '#' in a notebook body (a route-only '@nb' list or a \
'@parent^id@ %n' item) instead opens context 'notebook_section_bullet': \
the replacement covers only the selector after '#', rows share the glued \
notebook row model (kind, group, line, note_count, detail, consequence, \
creates, refused) plus the nested '  - ' continuation and cursor_after, \
groups order notebook, draft, then the create row, and an additive \
place_menu object on drawer, notebook section, and section-bullet \
responses carries the client copy (title, caption, groups, new_row_prefix, \
empty_hint, results_label, footer_hint). Pomodoro-name completion covers '@route:id#prefix', a bare \
'@route:id#', '@route:#prefix', '@route^id@#prefix', and a bare '@route^id@#'; \
Today rows come from `bob capture-pomodoros`: link-name completion offers \
open entries, while start-name completion also offers completed names as \
`again` rows. Duplicate theme keys collapse without merging distinct names \
that happen to share a selector slug. Both contexts compose their menu as \
Today matches, Recent exact/prefix matches, an optional create row, Recent \
substring matches, then utility rows. Recent comes from canonical daily \
notes, excludes hidden themes and components already represented by Today, \
and is capped at five rows for an empty query or twelve otherwise \
(twelve for an empty query too when a `themes:` vocabulary is configured, \
so the whole vocabulary can show). Hidden \
themes still work when typed and still appear in Today. An exact Today or \
Recent match suppresses a separate create row; a prefix match does not. \
A create row sets creates_pomodoro and replaces the query with the canonical selector; \
accepting it edits only the draft, and `bob capture` creates the named placeholder later.\n\n\
Recent rows set origin=history, last_used, days_used, and creates_pomodoro. \
Named rows expose their component themes and hues. A create row may set \
hidden or similar_to to explain an existing preference or nearby name. \
Replacements preserve the displayed name, using an exact-name quoted literal \
when a plain selector would change it or select another theme. Partial quoted \
names match theme-key prefixes; plain names also match slug substrings.\n\n\
Pomodoro-start-name completion covers `=name`, `=<X>name`, `=name<X>`, \
`=<X>#name`, and the `=<X>#` incomplete state. `<X>` accepts one `-` earlier \
or `+` later offset with an optional count; a lone sign means one 5-minute unit. \
A planned placeholder that a bare \
`=` would start carries next_up. Nameable utility rows set requires_name \
and use an empty replacement that clients must not insert; the running entry \
is last in start-name completion. Hashed-name replacements include `#`, \
and candidate formatting keeps it only for names that need it. A missing daily note, a missing Pomodoros \
section, and multiple open timed Pomodoros stay write-free warnings without \
a create row. On a `@<route>:<block-id>[#<name>]=<X>` marker the `=<X>` start suffix is \
never completable: block and Pomodoro-name replacement ranges end before \
the `=`, a cursor inside the suffix returns an empty success, and accepting \
a candidate preserves the typed suffix. The `=x[<N>][*<P>][!<M>][~<K>]` close suffix behaves the \
same way: it is never a completion field, replacements still stop before \
`#`/`=`, and a cursor anywhere inside the suffix, including the task-number \
lists and a dangling `,`/`~` separator, returns an empty success. A whole-item \
`+[N]`/`-[N]` Pomodoro adjustment, `++[N]`/`--[N]` Pomodoro shift (a bare `+`, `-`, `++`, or `--` is one unit), `x[<N>][*<P>][!<M>][~<K>][@<N>]` close, or a bare `=`/`=<X>` start (a bare `=` starts 25 minutes) is an action and requests no route or task completion candidates: a cursor on such an item returns an empty success. Work Log text completes like a bullet line whether it sits below the close or on it: inline entry text requests no marker, `:`/`^`, or `wikilink_block` candidates, while note and heading wikilinks keep completing. A `=<X>#name` or `=name` named start instead completes its name as `pomodoro_start_name`, per token inside chains; a cursor on timing or at the `#` byte itself returns an empty success, hashed-name replacements include `#`, candidate formatting keeps it only for names that need it, and a trailing glued `<X>` with no leading count stays in the replacement so acceptance keeps the typed timing (`=de|3` plus DEEP WORK yields `=3#deep-work`); a `~<K>` drop part stays outside the replacement. A cursor inside either suffix, including a dangling `~`/`,` separator, returns an empty success. A whole-item `x=` past record completes its name as `pomodoro_past_name` after `x=`, `x=#`, and `x=<D>#` and inside `x=#\"…`, per token inside chains: open placeholders and completed names are offered with the next untimed placeholder marked next_up, the running session (or a row resolving to it) is never offered, no `override` object is reported, and the replacement covers the name plus a trailing glued count when there is no leading count, ending before the repair `!`, a second `=`, or `@<D>`; a cursor on the count, the `!`, or the close part after the second `=` returns an empty success. A cursor inside its segment suffix completes as `pomodoro_past_segment`: the replacement covers the suffix range only, the query is the text after the first `~` up to the cursor, rows come from today's timeline in tail, `~1…~N`, head order, and accepting a row inserts the suffix only, never `!`. Pomodoro block-ID \
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
each group. A terminal bare `+query` on an item parent line or eligible \
authored child line uses the `task_parent` context: it searches all linkable \
open tasks across the vault, returns the stripped query and a server-authored \
picker descriptor, and replaces the whole selector. ID-less rows carry \
`requires_block_id` plus suggested IDs and an empty replacement, so clients \
must offer Add block ID rather than insert the row. A lone `+` is the \
intentional dual-use case: completion opens this picker and its descriptor \
returns the action continuation keys `0` through `9` and `+`, allowing an \
editor to preserve the Pomodoro adjustment when the user types a comma, as \
`action_continuation_aliases {\",\": \"+\"}`. Parent Task pickers \
also return `continuation: \" +\"` only when the accepted row \
followed by a comma would open the picker for another target. Scoped \
`@route+id` and \
`@route+id` keeps the `task` context, with picker scope and removal ranges \
identifying the note-local parent marker. The parent-task picker has \
complete `&` or `*` modifiers on its item, `picker.relationship` reports the \
resolved relationship rows (`raw`, `note_path`, `block_id`, and optional `text`). \
Self-edges, dependency cycles, and project-resident conflicts set \
`disabled_reason` and an empty replacement; shell completion omits those rows. \
For `task_dependency` and `blocked_task`, `owner` adds the resolved task's \
`text`, `note_path`, `status_symbol`, and `status_name`; a new-task owner carries \
its non-empty draft text. Unresolved owners keep the lexical owner fields. A \
draft with no complete relationship modifiers keeps the existing response shape. \
A `~` / `~~` modifier opens `relation_to` / `relation_from` over the full \
task catalog. Rows report `already_linked`, an existing label when present, \
and `also_prerequisite`; read-only sources and malformed source LINKS lines \
are disabled in `relation_from`. A `|label` field opens `relation_label`, whose \
suggestions count labels across all LINKS lines and return canonical `|label` \
or `|\"multi word\"` replacements. A `||label` probe keeps the `||` spelling \
in `replacement`, gains `one_off: true`, and every row also carries \
`hide_replacement` (always the `||` form). A typed invalid label stays first with a \
`disabled_reason` and an empty replacement. \
Configured cancel, complete, merge, and successor labels mark rows with \
`cancels_source`, `completes_source`, `merges_source`, and `offers_source`; \
zero-use configured labels are overlaid unless hidden. A configured merge \
label disables its row when the source is new or recurring. A configured successor label or its \
inverse spelling adds `relation_successor` with the offer verdict, an already- \
released caveat, or a `starts before` hint. \
When configured, the response includes `relation_cancel` and/or \
`relation_complete` with the source task, verdict, and consequence message, \
and `relation_merge` with the source task, merge verdict, and consequence \
message. \
A solo leading '^' token completes active tasks instead: the \
`route:block-id` part offers In Progress and Next tasks with block IDs, \
ordered by today's open-Pomodoro Task Links (queued first, then In \
Progress, then Next), and accepting a row inserts the full \
`route:block-id` in one step while a typed `#name`/`=<X>` suffix \
survives. A `#name` after `^route:block-id` completes Pomodoro names \
exactly as it does after `@route:block-id`, and a cursor inside `=<X>` \
or `=x[<N>][*<P>][!<M>][~<K>][@<N>]` offers nothing. A whole-item `!` token completes \
`task_complete`: every open task (Ready, Blocked, Next, In Progress) in the vault-wide \
completable catalog, today rows first and ranked by the query with today matches on top; \
the replacement covers the whole `!` token, sigil included, and the response carries a \
vault-scoped `picker` descriptor plus the `!`/`[` continuation keys on a bare `!`. \
A marker-position `#<type>` token at either line edge completes `task_type`: the \
replacement covers the whole marker, `query` is the decoded type name, and \
`subject` says `new` or `existing`. A glued '#...' after `^id` is refused; a separate '#type' retypes the new task (subject `new`), and a separate '#type' on a marker-only '@route+id' retypes before ensuring Next (subject `existing`); a bare unattached '#' \
opens `task_type` with an empty query. Configured rows follow config order for \
an empty query; hidden types remain available on exact matches. Candidates \
carry Bob's theme refs, current and inherited annotations, and a `set`, \
`clear`, or `keep` effect. An unconfigured typed name appears as a refused \
candidate with `type_not_configured` and near-name suggestions; shell completion \
never inserts it. The response includes the subject's \
`theme_suggestion`, and zsh groups the rows as `task types`. \
A bare `%` opens the `directive` context with Bob-authored Clip, Pomodoro \
note, and Template rows; bare-URL items also offer Listen. A typed letter \
filters the rows, and opening the menu never reads the clipboard. Bare `#` \
opens `task_type` with no row preselected. Record field-name completion covers a fresh first-level `- ` bullet \
and a partial `- sta` key on a claimed record item (required keys first, \
schema order within each group). Record value completion covers the caret \
inside a `key:: value` (and list member) and `-n/--next-missing-field`, \
which asks for the first missing required field in schema order. \
`-F/--record-field KEY` is mutually exclusive with `-n` and targets a \
declared field on the record item containing `--cursor`, even when no \
bullet exists: the value menu is unfiltered, a missing field inserts at \
the end of the field block, and a set field uses the existing \
value/member replacement (the member under the caret when the caret is \
in a list value, otherwise the final authored member). Named-field \
responses include `record.edit`, an explicit edit-in-draft action with \
Bob-authored replacement range/text and ordinary value-completion \
continuation (never next-missing). Satisfied \
defaults count as present; an explicit empty required field stays missing. \
Accepting a field name lands in `key:: ` and requests ordinary value \
completion; accepting a route, create row, or value requests the next \
missing field when any remain. Candidate `continuation` is that follow-up \
request and is separate from picker `continuation`. Typed record values \
own `%`, `#`, `@`, `~`, URLs, and `((` before task pickers. A local \
`@eat#` section menu prepends a `record_create` row on an empty query. \
Record parse objects and record completion metadata advertise \
`assist_version: 1` when this named-field contract is present.\n\n\
Every success response reports \
`grammar: 26` while `schema_version` stays 1; an empty bare `#` or `%` \
query, an empty `=*` merge source, or an empty drawer query after `@route+id#`, adds \
`initial_selection: \"none\"`. \
A solo leading `:` token \
completes `task_link`: every linkable open task (Ready, Blocked, Next, In \
Progress) in the routable inbox, area, and non-terminal project notes, in \
canonical picker order and ranked by the query with ID-less tasks always \
included; the replacement covers the whole `:` token, sigil included, \
because accepting rewrites the query into the canonical `@route:block-id` \
link. A whole-draft `/` token completes `task_goto`: the same rows and order as `task_link`, with `note_path` and absolute `target` for opening the task; the replacement covers the whole token and is never inserted. Pass -C/--closed-tasks to list only done and canceled tasks from the whole vault, instead of the open rows, as `completed` rows with the newest close date first. The right-hand side of '@route^block-id' completes as `task_block_id` once the route resolves, with empty \
candidates and an additive `block_id` object carrying intent, used IDs, and suggestions. A project-note `@` seal \
directly after the `^` or `:` block-ID part is never part of the replacement and reports intent `project_note`; a `+` after a `:` block ID is the retired project-note form and offers nothing. A trailing ` :id` or ` ^id` on a \
first-level bullet of a project-note item completes as `project_task_block_id`, with empty candidates and an additive `block_id` object carrying intent `new`, the project-note stem, the sigil range, the owning task body, used IDs (`prj` plus other task IDs), and suggestions. This prompt also opens on task-list bullets and nested lines, and on nested lines under project-note task bullets. Its object reports `owner_line` and an additive `splice`; list IDs use the header route, destination-note IDs followed by sibling-list IDs, and owner-first suggestions when that ID is free. The splice moves an accepted ID onto its owner line while preserving nested text and returning the caret to the typed line. An empty block-ID component \
('@route+#') returns a successful empty drawer list; an unresolvable \
parent task returns a successful empty list plus one bounded warning. Other \
contexts still rank \
exact prefix matches before substring matches, case-insensitively, while \
keeping each discovery source's stable order. Drawer rows come in three \
groups: `task` (the target's drawers), `recent` (catalog drawers the task \
lacks), and one `new` row for a typed name nothing claims; each group ranks \
key- or slug-prefix matches first, then substring matches, in document order \
inside each tier. Rows carry Bob-authored `detail` and `consequence` copy, \
`creates`, `cursor_after`, and a `refused` reason straight from the commit's \
resolver; the response adds `target` and the decoded `query`. On a `>` \
transfer line the same contexts serve each component: a source completes \
routes and that note's open tasks, narrowed to movable-or-mergeable rows \
(ID-less rows never list, since a source needs `+id`); `> @` completes \
routes; a complete bare destination keeps the route list and adds a \
`type + to merge` hint warning; `@c+` completes the destination note's \
open tasks minus the typed sources, each row carrying a `⤵ merge into` \
consequence; `@c+id#` completes that task's drawers through the usual \
drawer menu; and `@c#` completes the note's eligible `##` sections in \
document order, then a creation row for an unmatched name, while a \
strict-prefix or edit-distance-1 near-duplicate warns \
`did you mean `## …`?` instead of creating. The arrow, the gaps around \
it, and trailing text past the destination complete nothing.\n\n\
At `> +` or `> +query`, the vault-wide `task_parent` picker drops typed source \
tasks and ID-less rows; each remaining identified task carries its merge \
consequence. Accepting one replaces the selector with `@route+id`.\n\n\
On a first-level `- ^` or `- :` bullet in a sealed project-note item (`@<route>^<id>@` or `@<route>:<id>@`), \
the `project_task_move` context lists the parent's open top-level tasks, \
including ID-less rows that require the explicit Add block ID flow. The \
query contains the ID prefix and replacement covers only the ID. The response \
adds `project_task_move` parent/destination metadata and a note-scoped picker \
descriptor with Bob-authored marker ranges and the shift-return continuation. \
Rows carry stale-safe refs, today's Pomodoro chips, schedule and hidden state, \
Bob-authored consequences, and disabled reasons; the picker never scans the vault.\n\n\
When the cursor is inside an Obsidian wikilink, wikilink completion takes \
precedence over capture-marker completion. Note completion covers `[[note` \
and offers Markdown note paths, stems, and aliases. Heading and block \
completion cover target-qualified links like `[[note#Head` and \
`[[note#^block`, same-destination links like `[[#Head`, and vault-wide \
searches like `[[##Head` and `[[^^block`. Candidate replacements own the \
missing closing delimiter when needed and report the final cursor offset. \
A `((query` token in free text opens `wikilink_task`: every task in the \
vault-wide catalog (open and completed history, including ID-less rows), \
grouped this-note / In Progress / Next / other open notes / completed, \
with backend-formatted `[[target#^id]]` replacements, `link_target`, \
`cursor_after`, disabled reasons, and a vault-scoped picker descriptor \
(`kind: wikilink_task`, `scope_token: \"((\"`). Shell completion returns \
nothing for this field. The replacement covers the `((query` token \
(including a closed `((query))`), and `query` is the bytes after `((` \
up to the cursor.",
        )
        .after_help(
            "Examples:\n  bob capture-complete --cursor 1 --format json -- '@'\n  bob capture-complete -c 4 -- '@@fo'\n  bob capture-complete -c 20 -- 'Buy milk @@gro'\n  bob capture-complete -c 19 -f json -- 'jot idea @notes#Id'\n  bob capture-complete -c 20 -f json -- 'Fix flaky test @sase^'\n  bob capture-complete -c 12 -b ~/bob -- 'Do work @Dev^new-id'\n  bob capture-complete -c 16 -b ~/bob -- 'Do work @Dev:foc'\n  bob capture-complete -c 16 -b ~/bob -- 'note @foo+bar#'\n  bob capture-complete -a -c 6 -f json -- '@file+'\n  bob capture-complete -a -c 8 -f json -- '@@file'\n  bob capture-complete -c 5 -- '[[sas'\n  bob capture-complete -c 1 -- '^'\n  bob capture-complete -c 1 -- ':'\n  bob capture-complete -c 1 -f json -- '/'\n  bob capture-complete -C -c 1 -f json -- '/'\n  bob capture-complete -c 13 -f json -- 'Follow up ((s'\n  bob capture-complete -c 7 -- '@inbox+ > @db'\n  bob capture-complete -c 21 -f json -- '@inbox+bench > @db+up'\n  bob capture-complete -c 16 -f json -- '@inbox+bench > +'\n  bob capture-complete -n -c 16 -f json -- 'Ramen Nagi @eat/'\n  bob capture-complete -F visited -c 16 -f json -- 'Ramen Nagi @eat/'\n\nContexts:\n  route, section, record_field, record_value, pomodoro_block_id, task_block_id, project_task_block_id, project_task_move, pomodoro_name, pomodoro_start_name, pomodoro_past_name, pomodoro_past_segment, task, drawer, active_task, task_type, directive, task_link, task_goto, task_parent, task_dependency, blocked_task, relation_to, relation_from, relation_label, task_complete, wikilink_note, wikilink_heading, wikilink_block, wikilink_task",
        )
        .disable_help_flag(true)
        .arg(all_tasks_arg())
        .arg(bob_dir_arg())
        .arg(closed_tasks_arg())
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(next_missing_field_arg())
        .arg(record_field_arg())
        .arg(text_arg())
}

pub(super) fn all_tasks_arg() -> Arg {
    Arg::new("all-tasks")
        .long("all-tasks")
        .short('a')
        .action(ArgAction::SetTrue)
        .help(
            "Include open tasks that still need a block ID (task context only)",
        )
}

pub(super) fn closed_tasks_arg() -> Arg {
    Arg::new("closed-tasks")
        .long("closed-tasks")
        .short('C')
        .action(ArgAction::SetTrue)
        .help(
            "List done and canceled tasks from the whole vault instead of open tasks (task_goto context only)",
        )
}

pub(super) fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

pub(super) fn cursor_arg() -> Arg {
    Arg::new("cursor")
        .long("cursor")
        .short('c')
        .value_name("BYTE")
        .required(true)
        .value_parser(clap::value_parser!(usize))
        .help("UTF-8 byte offset of the cursor within TEXT")
}

pub(super) fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

pub(super) fn next_missing_field_arg() -> Arg {
    Arg::new("next-missing-field")
        .long("next-missing-field")
        .short('n')
        .action(ArgAction::SetTrue)
        .conflicts_with("record-field")
        .help(
            "Offer the first missing required field of the record item containing the cursor",
        )
}

pub(super) fn record_field_arg() -> Arg {
    Arg::new("record-field")
        .long("record-field")
        .short('F')
        .value_name("KEY")
        .conflicts_with("next-missing-field")
        .help(
            "Offer values for declared record field KEY on the item containing the cursor, even when no bullet exists",
        )
}

pub(super) fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

pub(super) fn text_arg() -> Arg {
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

impl super::model::OutputFormat {
    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
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

pub(super) fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
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
pub(super) fn raw_text_from_matches(
    matches: &ArgMatches,
) -> Result<String, String> {
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
