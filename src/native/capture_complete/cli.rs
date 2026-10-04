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
    engine::build_result,
    model::{CompleteError, OutputFormat},
    render::{print_error, print_success},
};
use crate::native::env as bob_env;

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

pub(super) fn print_clap_error(error: clap::Error) -> i32 {
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
each group. A terminal bare `+query` on an item parent line or eligible \
authored child line uses the `task_parent` context: it searches all linkable \
open tasks across the vault, returns the stripped query and a server-authored \
picker descriptor, and replaces the whole selector. ID-less rows carry \
`requires_block_id` plus suggested IDs and an empty replacement, so clients \
must offer Add block ID rather than insert the row. A lone `+` is the \
intentional dual-use case: completion opens this picker and its descriptor \
returns the action continuation keys `0` through `9` and `+`, allowing an \
editor to preserve the Pomodoro adjustment. Scoped `@route+id` and \
`@@route+id` keep the `task` context, with picker scope and removal ranges \
identifying the note-local parent marker. A solo leading '^' token completes active tasks instead: the \
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
            "Examples:\n  bob capture-complete --cursor 1 --format json -- '@'\n  bob capture-complete -c 4 -- '@@fo'\n  bob capture-complete -c 20 -- 'Buy milk @@gro'\n  bob capture-complete -c 19 -f json -- 'jot idea @notes#Id'\n  bob capture-complete -c 20 -f json -- 'Fix flaky test @sase^'\n  bob capture-complete -c 12 -b ~/bob -- 'Do work @Dev^new-id'\n  bob capture-complete -c 16 -b ~/bob -- 'Do work @Dev:foc'\n  bob capture-complete -c 16 -b ~/bob -- 'note @foo+bar#'\n  bob capture-complete -a -c 6 -f json -- '@file+'\n  bob capture-complete -a -c 8 -f json -- '@@file+'\n  bob capture-complete -c 5 -- '[[sas'\n  bob capture-complete -c 1 -- '^'\n  bob capture-complete -c 1 -- ':'\n\nContexts:\n  route, section, pomodoro_block_id, task_block_id, project_task_block_id, pomodoro_name, pomodoro_start_name, task, task_section, active_task, task_link, task_parent, wikilink_note, wikilink_heading, wikilink_block",
        )
        .disable_help_flag(true)
        .arg(all_tasks_arg())
        .arg(bob_dir_arg())
        .arg(cursor_arg())
        .arg(format_arg())
        .arg(help_arg())
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
        .help("Capture text; multiple args are joined with spaces")
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
