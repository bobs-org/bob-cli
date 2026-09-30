//! CLI construction and request parsing for bob capture.
use super::*;

pub(super) fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

pub(super) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Capture a task or bullet into the Bob vault")
        .long_about(
            "Capture one or more tasks or bullets into the Bob Obsidian vault.\n\n\
TEXT is split into ordered capture items by one or more blank or whitespace-only \
physical lines; leading, trailing, and repeated separators are ignored. A \
whitespace-separated line of only Pomodoro session operators is also split \
into one item per operator, so `+2 =x` means `+2`, blank line, `=x`. A \
'@@<route>' or '@@<route>+<block-id>' declaration token anywhere in the draft \
is metadata, not a capture item: every otherwise-unrouted item inherits that \
destination, while an item-local @route, @route+id, @route#..., @route^..., \
@route:..., or trailing '#' marker still wins for that item. A declaration-only draft \
fails. Do not combine a textual @@ declaration with --route, --section, --task, \
or --task-section. Each item's first nonblank line is normalized and \
becomes that item's parent, formatted as a #task with a [created::] stamp unless a \
bullet or sub-bullet route is selected, and written to mac_inbox.md unless an \
@route token, @@ declaration, or --route target is provided. Existing target files \
prefer a Tasks section, then fall back to the last top-level task block. Missing \
target files are created when needed.\n\n\
Within each item, every later physical line must be either a column-zero \
Markdown bullet or a nested bullet prefixed by exactly two ASCII spaces. At \
either depth, '-', '*', or '+' must be followed by a space or tab; the source \
marker and separator are stripped and the item is rendered with the canonical \
'- <body>' marker. Column-zero authored items render one target-selected \
indentation unit beneath the parent; two-space source items render two units \
beneath the parent and attach to the nearest preceding nonempty column-zero \
authored item. A marker-only placeholder is skipped and never clears that \
owner. Unsupported indentation, prose continuation, or an orphaned nested \
item fails with a usage error naming the physical line, and so does an item \
left with no text once its own capture markers are removed. Every recognized \
terminal 's:<N>', 'p:<N>', '%...', and '@route' marker configures only its own \
capture item and is stripped from the rendered line it was typed on; a second \
line in the same item resolving the same marker is ambiguous and fails before \
anything is written. Only an item's first line keeps the established leading \
'@route text' form. Authored children render before any clipboard children \
and the priority schedule log.\n\n\
All items are planned against in-memory note snapshots before commit. Later \
items see earlier planned edits to the same target, and any parse, clipboard, \
validation, staging, or replace failure leaves notes, ledgers, and newly \
created clipboard files at their original state. Single-item JSON keeps its \
legacy shape; multi-item JSON keeps the first result at the top level and adds \
an ordered 'captures' array.\n\n\
Append a trailing lowercase 's:<N>' token, where N is a non-negative integer, \
to schedule the capture N days from today. The token is removed from the task \
text and rendered as [scheduled::YYYY-MM-DD] after [created::YYYY-MM-DD]. It \
may appear before or after a trailing @route token and is recognized only at \
the very end of the input. Checkbox-bearing captures with a resolved scheduled \
property start Blocked as '[?]', including s:0; ordinary bullet and sub-bullet \
forms stay checkbox-free.\n\n\
Append a trailing lowercase 'p:<N>' token, where N selects the Nth priority \
level configured in ~/.config/bob/config.yml (1-4 today: P1-P4), to write \
[priority::<value>] and roll a random [scheduled::YYYY-MM-DD] inside that \
level's day window. A task with no priority field is implicitly P0, so there \
is no p:0. An explicit s:<N> wins the scheduled date and p:<N> still writes \
the priority. Like s:<N> it is recognized only in the terminal token region \
and may appear on either side of a trailing @route token.\n\n\
Append a trailing clipboard marker: '%' captures one live value without a \
header; '%<positive integer>' captures exactly that many values without \
headers, starting with the live clipboard and then recent history newest first; \
and '%<nonnumeric header>' captures one live value under an explicit header. \
'%1' is equivalent to '%', while '%0' stays literal. Headers use letters, \
digits, '_' and '-'; '_' renders as a space. The marker composes with s:<N>, \
p:<N>, and every route kind in either terminal order. Each value is classified separately: \
small text stays inline; 2-10 flat text lines and 1-10 flat unordered Markdown \
list items become child bullets, with source list markers removed; copied file \
paths are saved under img/ or file/; and long or other Markdown-structured text \
is preserved in a timestamped file/clip-*.md snippet. Clipboard children use \
the target note's dominant tab-or-two-space indentation and fall back to a tab. \
History captures fail without writing \
unless the exact requested count succeeds. Use --clip[=HEADER] to force one live \
capture while keeping '%' tokens literal; --clip=<digits> requests a numeric \
header. Bare --clip also captures without a header. Use --no-clip to keep a \
genuine trailing '%...' token literal. Clipboard failures abort before the note \
or attachment files are changed.\n\n\
Use '@<route>:<block-id>' in the same leading or trailing position to create \
a next-status task and link it from today's Pomodoro ledger. The routed task \
renders as '- [*] #task <body> [created::YYYY-MM-DD] ^<block-id>' when \
unscheduled, or '[?]' with any scheduled property before the final block ID. \
An optional '#<pomodoro>' slug, as in '@<route>:<block-id>#<pomodoro>', \
first targets a named open Pomodoro instead of the implicit current-or-future \
choice; whole-slug matches beat earlier prefix matches, and a typed '#' with \
an empty name is incomplete. If no open name matches, capture creates a \
canonical named future entry as '- [ ] () — NAME' and puts the task link \
beneath it. The daily note comes from \
BOB_DAY_FILE or <bob-dir>/YYYY/YYYYMMDD.md. Unnamed capture prefers the single open \
timed entry in its Pomodoros section and otherwise uses the first open entry. \
Named creation inserts after the current Pomodoro's complete block, else after \
the last completed Pomodoro's complete block, else before the first Pomodoro. \
Existing named matches skip the multiple-open-timed guard; named creation uses \
that guard because it needs an unambiguous current anchor. \
Both notes are fully validated before either is replaced; duplicate block IDs, \
duplicate Pomodoro links, missing ledger structure, no eligible unnamed target, \
invalid Pomodoro names, and multiple open timed entries for implicit or named \
creation fail \
without a partial capture. Append '=<X>' to start the session atomically, as in \
'@<route>:<block-id>=3' or '@<route>:<block-id>#<pomodoro>=-2': '<X>' mirrors the \
'se<X>' snippet (empty, digits, '-', '-digits', or 'digits-' with optional digits; \
empty duration means 25 minutes, bare '-' means 5-minute offset, no '-' means zero \
offset). The start replaces the selected untimed '()' placeholder with \
'(**HHMM-HHMM** [t:: Nm])' computed from BOB_NOW at 5-minute rounding, then links \
the new task beneath it; any open timed entry fails with 'finish the current \
Pomodoro first'. The started entry moves ahead of every open Pomodoro, right after \
the last completed one. For '=', the first open placeholder wins, else an unnamed entry \
is created; for '#name=', an open name match wins, else a canonical named entry is \
created. '=<X>' cannot combine with 's:<N>' or 'p:<N>'.\n\n\
Capture a whole item '@<route>:<block-id>[#<pomodoro>][=<X>]' to link an \
existing task into today's Pomodoro ledger instead of creating one, as in \
`bob capture '@sase:deep-fix'` or `bob capture '@sase:deep-fix#bugs=-'`. \
The marker must be the entire item: no body text, authored children, '%', \
's:<N>', 'p:<N>', or forced destination flags. Ready and Blocked tasks \
become Next (a valid future schedule is retired with a pull-forward log \
entry); Next and In Progress tasks keep their status. With '#<pomodoro>' \
the link moves to that named entry (creating it when needed); without a \
name an already-queued task stays in its Pomodoro and '=<X>' starts that \
entry and moves it to the front of the queue, otherwise the implicit current/next entry is used. The \
'^' spelling ('bob capture '^sase:deep-fix='') executes identically and \
exists so typing '^' completes In Progress, Next, and Ready `#now` tasks. JSON \
reports a distinct 'pomodoro_link' kind with the status transition, the \
ledger action (linked, moved, or already_current), and the resolved \
destination.\n\n\
A single-token, single-line item starting with ':' (for example ':' or \
':dee') is a task-picker query, never a capture: it fails with a teaching \
error so an unfinished query cannot create a junk inbox task. Accepting the \
picked task in a picker inserts its '@<route>:<block-id>' link, which \
captures exactly like a typed link.\n\n\
Capture a whole item `+N` or `-N` (for example `+5` or `-2`) to adjust \
today's current timed Pomodoro by N five-minute units: `+5` extends by 25 \
minutes, `-2` shortens by 10 minutes. The count is optional and defaults \
to 1, so `+`, `-`, `++`, and `--` all work. The item must contain only the \
signed count (leading/trailing whitespace is fine); any extra text, \
marker, or child line fails instead of creating a task, and `Plan +5` \
stays ordinary prose. `+0`/`-0` fail, and oversized magnitudes fail checked \
arithmetic before any write. The adjustment selects the single open timed \
entry in today's `## Pomodoros` section, keeps the start fixed, recomputes \
`new_end = (start + new_duration) modulo 24 hours` with `new_duration = \
max(0, old + signed_units*5)`, and rewrites only that ledger line to Bob's \
canonical `(**HHMM-HHMM** [t:: Nm])` form, preserving other metadata, child \
bullets, and newline style. A `@@` declaration still routes ordinary items \
in the same draft but never turns an adjustment into a task. Forced \
destination, task, section, and clipboard options (`--route`, `--section`, \
`--task`, `--task-section`, `--clip`, `%`, `s:<N>`, `p:<N>`) are rejected \
on adjustment items. Later items observe earlier staged edits to the same \
daily file, dry-run reports without writing, and any failure rolls the \
whole batch back. JSON reports a distinct `pomodoro_adjust` kind with an \
additive `pomodoro_adjust` object (direction, requested units/minutes, \
actual delta, before/after timing, line/name, rendered range); human output \
names what changed and where, and dry-run says what would change.\n\n\
Capture a whole item `++N` or `--N` (for example `++3` or `--2`) to shift \
today's running timed Pomodoro N five-minute units later or earlier, \
keeping its duration: both endpoints translate modulo 24 hours like \
Obsidian's `N\\o` / `N\\O`. The count defaults to 1 (`++` / `--` move one \
unit); `++0` / `--0` and oversized magnitudes fail before any write, and \
the item must contain only the operator. JSON reports a distinct \
`pomodoro_shift` kind with an additive `pomodoro_shift` object; human \
output names what shifted and where, and dry-run says what would shift. \
The shell sees a leading `-` as a flag, so spell an earlier shift as \
`bob capture -- --2` (or `bob capture --1` for one unit); a bare `--` \
stays the end-of-options marker and carries no text.\n\n\
Pomodoro sessions form a lifecycle: `=`/`=<X>` starts the next session, \
`=<X>#<pomodoro>` starts that named session, `+[N]`/`-[N]` resizes the \
running session, `++[N]`/`--[N]` shifts it, and `=x` stops the running one \
(`=` starts the next session, `=#name` starts that one, `=x` stops the \
running one). Session operators may share one line when whitespace \
separates them: `+2 =x` means `+2`, blank line, `=x`, and `=x =` closes \
then starts the next session. Quote `=` items in zsh, which expands a \
leading `=word` to a command path.\n\n\
Batches that change today's `## Pomodoros` section also report the plan \
budget: a top-level `plan_budget` object with before/after theme and link \
meters, an `added_themes` list, and cap warnings that fire only while the \
batch grows a meter past its cap. Human output prints one `plan T/Tc \
themes · L/Lc links` meter line after the result plus one `bob capture: \
warning: …` stderr line per fired warning, and names where each new Task \
Link landed (`→ into running GOALS (0945-1015)`, `→ under GOALS (next \
up)`, `→ under GOALS (named)`, or `→ new Pomodoro BOB`; JSON reports the \
same destination with a `role` of `current`, `next_up`, `named`, or \
`created`). With `plan.strict: true` in the Bob config, a batch that \
creates a new named Pomodoro past the theme cap is refused atomically \
(exit 1, JSON `code: plan_theme_cap_exceeded`); session starts are never \
refused. An invalid plan config skips the budget with one plain warning. \
See `docs/capture.md` (`Plan budget and strict mode`) and `docs/plan.md`.\n\n\
Capture a whole item `=`/`=<X>` (for example `=`, `=3`, `=-2`, `=2-1`) to \
start today's next future Pomodoro now with the same timing as the `se<X>` \
snippet: empty is 25 minutes, `3` is 15 minutes, `-` is 25 minutes with a \
5-minute offset, `2-1` is 10 minutes with a 5-minute offset. Write \
`=<X>#<pomodoro>` (for example `=#deep-work`, `=3#bugs`) to start the \
named Pomodoro now with `se<X>` timing: an open match (whole slug, else \
prefix) starts in place, a completed match starts a new session with that \
name (an \"again\" start), and otherwise a new named session is created \
and started. `<X>` goes before `#`; `=#bugs=3` teaches `=3#bugs`. Write \
a trailing `~<K>` drop list (for example `=~2`, `=3~2,4`, `=#bugs~2`, \
`=3#bugs~1,3`) to start without those queued Task Links: `~` drops, so \
`=~2` drops task 2 from the session you start the way `=x~2` drops task 2 \
from the session you stop, using the numbers the start lineup shows. The \
drop part always comes last, after `<X>` and after `#name` when present. \
The item must contain only the start token (leading/trailing whitespace \
is fine) and have exactly one physical line; a claimed token (counted or \
drop-carrying) with extra text, markers, or child lines (`=3 more`, `=3x`, \
`=~2 more`) and an exact token with child lines fail instead of creating \
a task, while a bare token with prose (`= foo`, `==`, `= ~2`) and mid-body \
tokens (`Plan =3`) stay ordinary prose. A dangling `~`/`,` (`=~`, `=~2,`) \
is incomplete and fails. A bare `=`/`=<X>` start needs a future \
`- [ ] ()` placeholder; a named start creates its session when no open \
entry matches. Both forms refuse while a timed entry is running; a running \
session names itself and teaches the `=x`-then-`=` switch idiom \
(`=x =#name` switches sessions in one line). The started entry moves to \
the current slot and reports its queued Task Links, numbered 1..N in \
ledger order with a numbered human index column. A `@@` declaration \
never applies to start items, and forced \
destination/task/section/clipboard options are rejected on them. Later items \
see earlier staged edits, dry-run reports without writing, and any failure \
rolls the whole batch back, so `=x`, blank line, `=` switches sessions \
atomically.\n\n\
Capture a whole item `=x[<N>][!<M>][~<K>]` (case-insensitive `=X`, with \
`!` and `~` in either order) to close today's running timed Pomodoro the \
way Obsidian's Ctrl+Enter completion does, plus an auto-decrement that \
shortens an early-stopped session to the earliest five-minute step at or \
after now (never extended; an overrun is reported). A bare `=x` keeps \
today's behavior; `=x<N>` keeps only the numbered Task Links in `<N>` in \
progress and defers the rest, `=x!<M>` completes the links in `<M>`, \
`=x~<K>` drops the links in `<K>` (removed from the closed session, not \
carried and not started; a dropped task keeps its lane), and combined \
forms do each part at \
once. `<N>`, `<M>`, and `<K>` are comma-separated task numbers in ledger \
order starting at 1 (the numbers `bob capture` shows, in human output, in \
`--dry-run`, and as JSON `task_links`); a lone `0` means no \
task stays in progress, as in `=x0`. The outcome is exactly the marker \
edits the user would make by hand before Ctrl+Enter, followed by the \
unchanged close. The item must contain only the close token \
(leading/trailing whitespace is fine); a token with extra text, markers, \
or child lines fails, a token ending in `,`, `!`, or `~` (`=x1,`) is \
incomplete and fails, whitespace is never allowed inside the lists, and \
`=xx`/`=xa`/`Plan =x` stay ordinary prose. `@route:block-id=x…` and \
`^route:block-id=x…` first put that existing task into the running session \
then close it, and `<text> @route:block-id=x…` creates the new task in the \
running session then closes it; numbers refer to the post-link lineup, and \
`#name` with a close fails because only the running session can close. \
Later items see earlier staged edits, dry-run reports without writing, and \
any failure rolls the whole batch back, so \
`printf -- '-2\\n\\n=x\\n' | bob capture` adjusts then closes atomically. \
JSON reports a distinct `pomodoro_close` kind (link and task forms keep \
their kind with an additive `pomodoro_close` object) carrying the typed \
`raw`, the `in_progress`/`complete`/`drop` lists, the numbered \
`task_links` lineup (with a `dropped` outcome), each task row's `index` \
and `role` (with a `dropped` role and a `now` flag for `#now` tasks); \
human output names the session, the range change, the file, and the line, \
prefixing numbered rows with their index, listing dropped rows (with a \
`stays <status>` lane caption), and summarizing `Dropped <K>`. \
Single-quote the argument: zsh expands a leading `=word` and `!` \
history expansion applies.\n\n\
Use '@<route>^<block-id>' in the same leading or trailing position to create \
an ordinary open task with the requested trailing Obsidian block ID, without \
creating or modifying a Pomodoro ledger link. It renders as '- [ ] #task \
<body> [created::YYYY-MM-DD] ^<block-id>' when unscheduled, or '[?]' with \
priority and scheduled properties before the final block ID. Duplicate block IDs in the destination \
note fail before the note is replaced; a missing destination note may still be \
created like any ordinary routed task. This form never reads or requires the \
daily note. The retired '@<route>::<block-id>' spelling is no longer accepted; \
use '@<route>^<block-id>' instead.\n\n\
Append '+' immediately after the block ID, as in '@<route>^<block-id>+' \
with an optional '#<pomodoro>' name ('@<route>^<block-id>+#<pomodoro>'), to \
create a brand-new sub-project note \
'<route>_<block-id with every '-' replaced by '_'>.md' at the vault root \
instead of a task. The note carries a 'parent: \"[[<route>]]\"' link, a \
'[[project]]' type with 'wip' status, and a '- [ ] #task #prj <body> #hide \
^prj' lifecycle task, mirroring the Obsidian 'Create project note from task' \
command. The '^prj' task is never linked and never starts as '[*]'. The route \
is lower-cased and the block ID keeps its authored case. \
The parent '<route>.md' must already exist as an area or non-terminal project \
note; the new note must not exist yet; the parent note itself is never \
modified ('bob projects sync' owns its Sub-projects line). Authored child \
bullets become '## Tasks' entries or '## Title Case' sections from ALL-CAPS \
titles in the new note. 's:<N>' and 'p:<N>' write frontmatter 'scheduled' and \
an inline priority on the '^prj' line, while '%...' and --clip are rejected. \
The '#<pomodoro>' name picks the Pomodoro that ' :<id>' Task Links go under; \
with no such task the name is rejected as unused. The retired '@<route>:<block-id>+' spelling is no longer accepted; \
use '@<route>^<block-id>+' instead. End a first-level task bullet with \
' :<block-id>' to name it, make it Next ('[*]', or '[?]' when the project is \
scheduled), and link '[[<stem>#^<block-id>]]' into the current/next Pomodoro \
or the named one; end it with ' ^<block-id>' to name it only. Named tasks \
render the ID last, after '[created::...]'. The project note and the daily \
note are written together, so a duplicate link or a missing daily note \
writes nothing.\n\n\
Use '@<route>+<block-id>' in the same leading or trailing position to capture \
an ordinary child bullet beneath an existing task without a [created::] stamp. It \
renders as '- <body>' and writes only the optional scheduled property. The complete \
new child is placed before the selected task's first direct-child Schedule Log or \
Work Log; if neither managed log exists, the child is appended at the end of the \
task block. The note and task must already exist. Existing child indentation and \
line endings are preserved; run 'bob capture-tasks -r <route>' to list eligible \
task block IDs. A trailing '#<section>' component, as in \
'@<route>+<block-id>#<section>', names an ALL-CAPS child section of that task. \
The selector may use A-Z, a-z, 0-9, and & ' ( ) , . / -. Whole-slug matches beat \
earlier prefix matches, so #future-work still wins over FUTURE WORKFLOW. The \
captured block is appended at the end of that section, before a managed log \
nested under it, using the section's child indentation. A selector that matches \
nothing is an error listing the task's real sections; capture never falls back \
to the end of the task. '@<route>+<block-id>#' with an empty selector is \
incomplete and needs a task section; run \
'bob capture-task-sections -r <route> -i <block-id>' to list them. A marker-only \
'@<route>+<block-id>' item with no body text ensures that task is Next and \
relocates its existing open-Pomodoro Task Link to today's implicit current/next \
Pomodoro without creating a missing link or toggling Next back to Ready. The \
same marker-only form with a Pomodoro selector, '@<route>+<block-id>#<pomodoro>', \
ensures Next and moves that existing Task Link subtree to the named open \
Pomodoro, or creates that named future Pomodoro and moves the subtree there. \
Neither unsuffixed form synthesizes a missing Task Link. A terminal '!' on the \
unsuffixed marker-only form, '@<route>+<block-id>!', is the only spelling that \
toggles the Pomodoro Task Link itself: when no matching link sits under an \
open Pomodoro the task links (Ready/Blocked rise to Next while Next/In \
Progress keep their lane), and when a matching link is already there every \
such link is removed with the lane unchanged; '!' cannot be combined with \
'#<pomodoro>'.\n\n\
Append a bare trailing '#' to capture the item as a plain-text sub-bullet on a \
Pomodoro instead of a task. It renders as '- <body>' with no [created::] stamp, \
no '#task' marker, and no block ID. The daily note comes from BOB_DAY_FILE or \
<bob-dir>/YYYY/YYYYMMDD.md. Capture prefers the single open timed entry in its \
Pomodoros section, otherwise the last completed entry, and otherwise the first \
open entry, appending the new bullet at the end of that entry's child block. \
The marker composes with \
'%...' and --clip but is rejected alongside 's:<N>', 'p:<N>', '@route', and \
--route.\n\n\
Append '#<section-prefix>' or a bare '#' to an @route token (such as \
'@notes#Ideas' or '@notes#') to capture an ordinary bullet instead. It renders \
as '- <body> [created::YYYY-MM-DD]' and is placed in a non-Tasks section whose \
heading title starts with the prefix (compared case insensitively), or any \
non-Tasks section for a bare '#'. A matching non-H1 section is preferred; a \
matching H1 heading is used only when no non-H1 heading matches. The marker may \
lead ('@notes#Ideas jot idea') or trail ('jot idea @notes#Ideas') the body. \
A standalone terminal '#<section-prefix>' marker (not appended to an @route \
token) is still not accepted and fails with a usage error; a standalone bare \
'#' is the Pomodoro-note marker described above, not a bullet marker.\n\n\
Use --route with --section to force bullet mode while keeping @tokens literal. \
The section title is matched exactly, case insensitively, against non-Tasks \
headings; if no heading matches, the bullet falls back to the pre-heading \
section.\n\n\
Use --route with --task and --task-section TITLE to nest a sub-bullet under \
the named ALL-CAPS child section of that task. The title is matched exactly, \
case insensitively; unlike a typed #<section> selector, it is not slug- or \
prefix-matched, so --task-section future-work does not match FUTURE WORK.",
        )
        .after_help(
            "Examples:\n  bob capture buy milk @groceries\n  bob capture buy milk s:1\n  bob capture buy milk s:2 @groceries\n  bob capture buy milk @groceries s:2\n  bob capture buy milk p:2\n  bob capture research rust p:4 @dev\n  bob capture buy milk %\n  bob capture research links %3\n  bob capture investigate %log @dev:blockid\n  bob capture --clip=screenshot -- save dashboard\n  bob capture '@dev^foobar' 'Some ordinary task.'\n  bob capture '@dev:foobar' 'Some foobar task.'\n  bob capture '@dev:foobar#bugs' 'Some foobar task.'\n  bob capture '@cash^goog-exit+' 'Finish the Google exit packet!'\n  printf 'Finish the Google exit packet! @cash^goog-exit+\\n- Draft the resignation memo\\n' | bob capture
  printf 'Finish the Google exit packet! @cash^goog-exit+#admin\\n- Draft the resignation memo :draft-memo\\n  - keep it short\\n- Collect the equity paperwork ^equity-docs\\n' | bob capture\n  bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'\n  bob capture '@cash+goog-exit!'\n  bob capture +5\n  bob capture -- -2\n  bob capture +\n  bob capture ++3\n  bob capture -- --2\n  bob capture --1\n  printf '+5\\n\\nCall bank @Cash+\\n' | bob capture\n  printf -- '--2\\n\\n+\\n' | bob capture\n  bob capture '=x'\n  bob capture '=x2'\n  bob capture '=x1!2'\n  bob capture '=x1~2'\n  bob capture '=x0'\n  bob capture '='\n  bob capture '=3'\n  bob capture '=~2'\n  bob capture '=3#bugs~1'\n  bob capture '=#deep-work'\n  bob capture '=3#bugs'\n  bob capture '=x =#bugs'\n  bob capture '=x =~2'\n  printf '=x\\n\\n=\\n' | bob capture\n  bob capture '+2 =x'\n  bob capture '=x ='\n  bob capture '^bob:capture-stop=x'\n  bob capture '^bob:capture-stop=x!1'\n  printf -- '-2\\n\\n=x\\n' | bob capture\n  bob capture 'Postgres 17 minimum @foo+bar#requirements'\n  bob capture --route foo --task bar --task-section REQUIREMENTS -- 'Postgres 17 minimum'\n  bob capture remembered to bump the timeout #\n  bob capture paste the failing output % #\n  bob capture jot idea @notes#Ideas\n  bob capture --route notes --section Ideas -- jot idea\n  bob capture @notes#Ideas jot idea\n  printf '@@foo\\nFirst task\\n\\nSecond task @bar\\n' | bob capture\n  printf '@@foo+a-id\\nFirst note\\n- authored detail\\n\\nSecond note\\n' | bob capture\n  echo 'buy milk @groceries' | bob capture\n  bob capture -f json -- @work send status\n  printf 'Prepare launch\\n- Confirm owner\\n\\nSend status @work\\n' | bob capture\n  printf 'Prepare launch\\n- Confirm owner\\n- Attach checklist\\n' | bob capture\n\nEnvironment:\n  BOB_CLIPBOARD_CMD          whitespace-split command that prints the live clipboard; overrides platform tools\n  BOB_CLIPBOARD_HISTORY_CMD  whitespace-split history command; receives count and prints a newest-first JSON array of strings\n  BOB_CONFIG_FILE            exact bullet-property config file; defaults to $XDG_CONFIG_HOME/bob/config.yml or ~/.config/bob/config.yml\n  BOB_DAY_FILE               exact daily note used by Pomodoro-linked capture\n  BOB_DIR                    Bob vault root when --bob-dir is omitted\n  BOB_NOW                    current date/time override\n  BOB_PRIORITY_ROLL_SEED     fixed seed for p:<N> rolls; unset means random\n  XDG_CONFIG_HOME            base config directory for BOB_CONFIG_FILE's default; defaults to ~/.config\n\nClipboard source order:\n  Live: BOB_CLIPBOARD_CMD; macOS pbpaste; Linux wl-paste or xclip/xsel; tmux show-buffer\n  History: BOB_CLIPBOARD_HISTORY_CMD; otherwise read-only Clipy SQLite on macOS; no automatic provider elsewhere",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(clip_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(no_clip_arg())
        .arg(route_arg())
        .arg(section_arg())
        .arg(task_arg())
        .arg(task_ref_arg())
        .arg(task_section_arg())
        .arg(text_arg())
}

pub(super) fn clip_arg() -> Arg {
    Arg::new("clip")
        .long("clip")
        .short('c')
        .value_name("HEADER")
        .num_args(0..=1)
        .require_equals(true)
        .conflicts_with("no-clip")
        .help("Capture the clipboard, optionally with HEADER")
}

pub(super) fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

pub(super) fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Plan and report without writing notes or clipboard files")
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

pub(super) fn no_clip_arg() -> Arg {
    Arg::new("no-clip")
        .long("no-clip")
        .short('n')
        .action(ArgAction::SetTrue)
        .conflicts_with("clip")
        .help("Keep trailing %... clipboard markers literal")
}

pub(super) fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Force the route to NAME.md and keep @tokens in text literal")
}

pub(super) fn section_arg() -> Arg {
    Arg::new("section")
        .long("section")
        .short('s')
        .value_name("TITLE")
        .conflicts_with_all(["task", "task-ref", "task-section"])
        .help("Force a bullet into the exact section TITLE; requires --route")
}

pub(super) fn task_arg() -> Arg {
    Arg::new("task")
        .long("task")
        .short('t')
        .value_name("BLOCK-ID")
        .conflicts_with_all(["section", "task-ref"])
        .help("Append beneath task BLOCK-ID; requires --route")
}

pub(super) fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .value_name("REF")
        .conflicts_with_all(["section", "task"])
        .hide(true)
}

pub(super) fn task_section_arg() -> Arg {
    Arg::new("task-section")
        .long("task-section")
        .short('S')
        .value_name("TITLE")
        .conflicts_with("section")
        .help("Nest under the exact task-section TITLE; requires --route and --task")
}

pub(super) fn text_arg() -> Arg {
    Arg::new("text")
        .value_name("TEXT")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(OsStringValueParser::new())
        .help("Task text; multiple args are joined with spaces")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
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

#[derive(Debug, Clone)]
pub(super) struct CaptureRequest {
    pub(super) bob_dir: PathBuf,
    pub(super) dry_run: bool,
    pub(super) forced_clip: Option<ClipRequest>,
    pub(super) forced_destination_flags: Vec<&'static str>,
    pub(super) forced_route: Option<String>,
    pub(super) forced_section: Option<String>,
    pub(super) forced_sub_bullet_target: Option<SubBulletTarget>,
    pub(super) forced_task_section: Option<String>,
    pub(super) no_clip: bool,
    pub(super) raw_text: String,
}

impl CaptureRequest {
    pub(super) fn from_matches(
        matches: &ArgMatches,
    ) -> Result<Self, CaptureError> {
        let forced_clip =
            matches.contains_id("clip").then(|| ClipRequest::Current {
                header: matches.get_one::<String>("clip").cloned(),
            });
        if let Some(ClipRequest::Current {
            header: Some(header),
        }) = forced_clip.as_ref()
            && !capture_clip::is_valid_header(header)
        {
            return Err(CaptureError::usage(
                "--clip HEADER must contain only A-Z, a-z, 0-9, '_' or '-'",
            ));
        }
        let forced_route = matches.get_one::<String>("route").cloned();
        let forced_section = forced_section_from_matches(matches)?;
        if forced_section.is_some() && forced_route.is_none() {
            return Err(CaptureError::usage("--section requires --route"));
        }
        let forced_sub_bullet_target =
            forced_sub_bullet_target_from_matches(matches)?;
        if forced_sub_bullet_target.is_some() && forced_route.is_none() {
            let option = if matches.contains_id("task") {
                "--task"
            } else {
                "--task-ref"
            };
            return Err(CaptureError::usage(format!(
                "{option} requires --route"
            )));
        }
        let forced_task_section = forced_task_section_from_matches(matches)?;
        if forced_task_section.is_some() && forced_route.is_none() {
            return Err(CaptureError::usage("--task-section requires --route"));
        }
        if forced_task_section.is_some() && forced_sub_bullet_target.is_none() {
            return Err(CaptureError::usage(
                "--task-section requires --task or --task-ref",
            ));
        }

        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            forced_clip,
            forced_destination_flags: forced_destination_flags(matches),
            forced_route,
            forced_section,
            forced_sub_bullet_target,
            forced_task_section,
            no_clip: matches.get_flag("no-clip"),
            raw_text: raw_text_from_matches(matches)?,
        })
    }
}

pub(super) fn forced_sub_bullet_target_from_matches(
    matches: &ArgMatches,
) -> Result<Option<SubBulletTarget>, CaptureError> {
    if let Some(block_id) = matches.get_one::<String>("task") {
        if !is_block_id(block_id) {
            return Err(CaptureError::usage(
                "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
            ));
        }
        return Ok(Some(SubBulletTarget::BlockId(block_id.clone())));
    }
    matches
        .get_one::<String>("task-ref")
        .map(|task_ref| parse_task_ref(task_ref).map(Some))
        .transpose()
        .map(Option::flatten)
}

pub(super) fn parse_task_ref(
    value: &str,
) -> Result<SubBulletTarget, CaptureError> {
    note_tasks::TaskRef::parse(value)
        .map(|task_ref| SubBulletTarget::Ref {
            line: task_ref.line,
            digest: task_ref.digest,
        })
        .ok_or_else(|| {
            CaptureError::usage("--task-ref must use <line>:<digest>")
        })
}

pub(super) fn forced_section_from_matches(
    matches: &ArgMatches,
) -> Result<Option<String>, CaptureError> {
    let Some(section) = matches.get_one::<String>("section") else {
        return Ok(None);
    };
    if section.trim().is_empty() {
        return Err(CaptureError::usage("--section must not be empty"));
    }
    Ok(Some(section.clone()))
}

pub(super) fn forced_destination_flags(
    matches: &ArgMatches,
) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if matches.contains_id("route") {
        flags.push("--route");
    }
    if matches.contains_id("section") {
        flags.push("--section");
    }
    if matches.contains_id("task") {
        flags.push("--task");
    }
    if matches.contains_id("task-ref") {
        flags.push("--task-ref");
    }
    if matches.contains_id("task-section") {
        flags.push("--task-section");
    }
    flags
}

pub(super) fn competing_destination_error(flags: &[&str]) -> String {
    let listed = match flags {
        [] => "--route".to_string(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} or {second}"),
        _ => {
            let (last, rest) = flags.split_last().expect("nonempty flags");
            format!("{} or {last}", rest.join(", "))
        }
    };
    format!(
        "a @@ global destination declaration cannot be combined with {listed}; they are competing document-wide destination controls"
    )
}

pub(super) fn forced_task_section_from_matches(
    matches: &ArgMatches,
) -> Result<Option<String>, CaptureError> {
    let Some(section) = matches.get_one::<String>("task-section") else {
        return Ok(None);
    };
    if section.trim().is_empty() {
        return Err(CaptureError::usage("--task-section must not be empty"));
    }
    Ok(Some(section.clone()))
}

pub(super) fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    let bob_dir = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    // Normalize once so the close vault and the batch planner key every
    // file the same way. A relative `-b` would otherwise double-join the
    // vault dir: `resolve_target` joins it onto the vault-relative path and
    // `read_latest` would join it again.
    if bob_dir.is_absolute() {
        bob_dir
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&bob_dir))
            .unwrap_or(bob_dir)
    }
}

pub(super) fn raw_text_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureError> {
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
        .map_err(|error| CaptureError::io(format!("read stdin: {error}")))?;
    Ok(text)
}
