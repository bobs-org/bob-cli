# Capture

`bob capture` writes tasks and bullets, queues bare public links as background
reference jobs, and manages Pomodoro sessions without opening desktop
Obsidian. The job confirmation says reading queue. The background job files
the note when it runs; a queued clip appears in `bob ref list` after the
host's intake drain (`bob ref scan -i`). Companion commands
parse in-progress drafts, complete markers and wikilinks, list routes and
tasks, assign block IDs, and name Pomodoros.
Bob Mac Capture is the macOS menu-bar frontend: it owns the hotkey and panel,
then delegates grammar, preview, completion, and vault writes to these `bob`
commands.

`bob capture --help` is the concise usage contract. This page is the full
workflow guide.

## Contents

- [Grammar at a glance](#grammar-at-a-glance)
- [`bob capture`](#bob-capture)
  - [Routing and insertion](#routing-and-insertion)
  - [Global destination declaration](#global-destination-declaration)
  - [Scheduling and priority](#scheduling-and-priority)
  - [Multi-item capture](#multi-item-capture)
  - [Bulk templates](#bulk-templates)
    - [Saved templates](#saved-templates)
  - [Authored sub-bullets](#authored-sub-bullets)
  - [Clipboard](#clipboard)
  - [Saving links to your reading queue](#saving-links-to-your-reading-queue)
  - [Task with a requested block ID](#task-with-a-requested-block-id)
  - [Pomodoro-linked tasks](#pomodoro-linked-tasks)
  - [Starting the session atomically](#starting-the-session-atomically)
  - [Linking and starting existing tasks](#linking-and-starting-existing-tasks)
  - [Picking any open task with ':'](#picking-any-open-task-with)
  - [Going to a task with '/'](#going-to-a-task-with)
  - [Adding prerequisites with '&'](#adding-prerequisites-with)
  - [Blocking existing tasks with '*'](#blocking-existing-tasks-with)
  - [Relating existing tasks with '&' and '*'](#relating-existing-tasks)
  - [Linking tasks with '~' and '~~'](#linking-tasks)
  - [Completing tasks with '!'](#completing-tasks-with)
  - [Routing inbox tasks out](#routing-inbox-tasks-out)
  - [Plan budget and strict mode](#plan-budget-and-strict-mode)
  - [Starting the next Pomodoro](#starting-the-next-pomodoro)
  - [Starting a named Pomodoro](#starting-a-named-pomodoro)
  - [Adjusting the current Pomodoro](#adjusting-the-current-pomodoro)
  - [Shifting the current Pomodoro](#shifting-the-current-pomodoro)
  - [Closing the running Pomodoro](#closing-the-running-pomodoro)
    - [Retired close spellings](#retired-close-spellings)
    - [Choosing each Task Link's outcome](#choosing-each-task-links-outcome)
    - [Logging work while closing](#logging-work-while-closing)
  - [Chaining session operators on one line](#chaining-session-operators-on-one-line)
  - [Project notes](#project-notes)
  - [Sub-bullets under existing tasks](#sub-bullets-under-existing-tasks)
  - [Task Link toggle](#task-link-toggle)
  - [Pomodoro notes](#pomodoro-notes)
  - [Section bullets](#section-bullets)
  - [Command-line options](#command-line-options)
  - [Input, stdin, and JSON output](#input-stdin-and-json-output)
  - [Interactive editor markers](#interactive-editor-markers)
- [`bob capture-parse`](#bob-capture-parse)
- [`bob capture-related`](#bob-capture-related)
- [`bob capture-rewrite`](#bob-capture-rewrite)
- [`bob capture-complete`](#bob-capture-complete)
- [Discovery commands](#discovery-commands)
- [`bob capture-templates`](#bob-capture-templates)
- [`bob capture-task-id`](#bob-capture-task-id)
- [`bob capture-pomodoro-name`](#bob-capture-pomodoro-name)

## Grammar at a glance

One capture item is a parent line plus optional authored child bullets. Blank
physical lines split a draft into multiple items; each item is planned before
anything is written, and any failure rolls the whole batch back.

| Marker | Meaning |
| --- | --- |
| Bare public URL | Queue a reference job under `mac_inbox` when the URL routing policy admits it; `-R, --no-ref` keeps it a task |
| `URL @route` or `@route URL` | Queue the reference under that route (aliases resolve; source `explicit`) |
| `URL %` | Queue the reference with companion audio requested (`ref.listen` true; same parent selection as a bare URL: default, `@@route` global, or `-r` forced); exact standalone `@` remains a compatible alias |
| `URL % @route` or `@route URL %` | Queue the reference with companion audio under that explicit parent (`@` also remains a compatible listen alias) |
| Bare URL with `-r route` | Queue the reference under that route (source `explicit`) |
| `@@route` | Shared task destination, anywhere in the draft, for otherwise-unrouted items; a plain declaration also routes default (routeless) refs (source `global`), while `@@route+block-id` keeps refs as tasks |
| `@@route+block-id` | Shared parent-task destination, anywhere in the draft, for otherwise-unrouted items |
| `@@template:` | Expand one capture item per row from the following `{N}` template; captures exactly like the expansion |
| `@route` | Write a task to `<route>.md` (default route is `mac_inbox`) |
| `@route#Section` | Write an ordinary bullet into a matching non-`Tasks` heading |
| `@route#` | Write an ordinary bullet into any non-`Tasks` heading |
| `@route^block-id` | Ordinary open task with a user-authored block ID |
| `@route:block-id` | Next-status (`[*]`) task plus a Pomodoro task link; a future schedule (`s:<N>`, N ≥ 1) starts Blocked (`[?]`) and unlinked |
| `@route:block-id#pomodoro` | Same, linked under a matching named open Pomodoro or a new named future Pomodoro |
| `@route:block-id=<X>` | Same, and atomically start the selected session; `<X>` mirrors the `se<X>` snippet (empty is 25 minutes) |
| `@route:block-id#pomodoro=<X>` / `@route:block-id=name` | Same under the named Pomodoro, starting that session (`@sase:deep-fix=bugs` links under `BUGS` and starts it, exactly like `@sase:deep-fix#bugs=`); under the shared suffix rule a link takes every spelling a lone start takes except drop lists (`=bugs@3`, `=3#bugs`, `=#bugs`) |
| `@route:block-id[#pomodoro][=<X>]` with no other text | Link the existing `^block-id` task in `route.md` into today's ledger (no new task); a task Blocked only by its own prerequisites drops them first; `=<X>` starts the resolved session atomically |
| `^route:block-id[#pomodoro][=<X>]` with no other text | Identical execution; `^` is the active-task spelling and completes In Progress and Next tasks |
| `:<query>` | Incomplete: pick any open task to link (`capture-parse` needs `task_link`); accepting inserts `@route:block-id`, and execution never captures it |
| `/<query>` | Incomplete: pick any open task to open in Obsidian (`capture-parse` needs `task_goto`); accepting opens the task, and execution never captures it |
| `+query` or terminal prose `+` | Incomplete: choose a parent task (`capture-parse` needs `task_parent`); accepting inserts `@route+block-id`, and execution never captures the unresolved selector |
| lone `+` | Opens the same task picker in an editor; Escape leaves the valid +5m Pomodoro adjustment unchanged |
| `&note:block-id` | Prerequisite task link: the captured (or explicitly selected) task depends on `note.md`'s `^block-id` task; nested notes use `&projects/foo:bar`, notes with spaces use `&"Shopping List":bar` |
| `&`, `&query`, `&note:` | Incomplete: the prerequisite picker is still open (`capture-parse` needs `task_dependency`); execution refuses with a teaching error |
| `^route:block-id &note:id` or `&note:id ^route:block-id` | Add a prerequisite to the existing open owner task; `^route:id` is a bare owner alias here and does not create a Pomodoro link |
| `&note:block-id` with no owner | Incomplete: a prerequisite is selected but no dependent is named (`capture-parse` needs `dependency_target`); type task text or `+` to choose an existing owner |
| `&note:block-id +query` | Incomplete: choose the existing owner with the `task_parent` picker; execution refuses until the selection is complete |
| `+query &note:block-id` | Incomplete: choose the dependent owner with the `task_parent` picker |
| `*note:block-id` | Blocked-task link: the named open task depends on the captured (or explicitly selected) task; nested notes use `*projects/foo:bar`, notes with spaces use `*"Shopping List":bar`. Quote `*` on the shell (`bob capture 'Ask Kelly *job:cleanup'`): zsh globs it |
| `X *`, `X *query`, `X *note:` | Incomplete: the blocked-task picker is still open (`capture-parse` needs `blocked_task`); a line-initial bare `*` is never this marker |
| `^route:block-id *note:id` or `*note:id ^route:block-id` | Make the named open task depend on the existing owner; `^route:id` is a bare owner alias and does not create a Pomodoro link |
| `*note:block-id` with no owner | Incomplete: a blocked task is selected but no owner is named (`capture-parse` needs `dependency_target`); type task text or `+` to choose an existing owner |
| `*note:block-id +query` | Incomplete: choose the existing owner with the `task_parent` picker; execution refuses until the selection is complete |
| `+query *note:block-id` | Incomplete: choose the blocking owner with the `task_parent` picker |
| `~note:block-id[\|"label"]` | Relation link: the captured (or explicitly selected) task links to `note.md`'s `^block-id` task, with an optional label; nested notes use `~projects/foo:bar`, notes with spaces use `~"Shopping List":bar`, labels use `~note:id\|see` or `~note:id\|"follow-up of"`. A `~note:id\|\|see` spelling stores the identical label and hides it from suggestions once the capture succeeds (a one-off). Quote `~` on shells that expand it |
| `~`, `~query`, `~note:`, `~~note:block-id`, `~note:id\|`, `~note:id\|\|` | Incomplete: the relation picker is still open (`capture-parse` needs `relation_target`); a label position needs `relation_label`; execution refuses with a teaching error |
| `~~note:block-id[\|"label"]` | Reverse relation: the named task links to the captured (or explicitly selected) task; the captured task is the target, so it is always minted; the named task's LINKS line must be editable |
| `^route:block-id ~note:id`, `~note:id ^route:block-id`, `^route:block-id ~~note:id` | Relate an existing owner; `^route:id` is a bare owner alias here and does not create a Pomodoro link |
| `~note:block-id` with no owner | Incomplete: a relation target is selected but no owner is named (`capture-parse` needs `dependency_target`); type task text or `+` to choose an existing owner |
| `!` | Incomplete: pick an open task to complete (`capture-parse` needs `task_complete`); accepting inserts `!<note>:<block-id>`, and execution never captures it |
| `!fix`, `!sase:`, `!"Shopping List` | Incomplete: still typing the completion query (`capture-parse` needs `task_complete`) |
| `!note:block-id` with no other text | Complete an existing open task (`capture-parse` mode `task_complete`); `bob capture` closes it exactly as `x!N` would, retires its Task Links, and unblocks dependents |
| `!note:block-id` plus anything extra | Claimed and invalid (`invalid_task_complete`); a completion must be the whole capture item, never a junk inbox task |
| `+[N]` / `-[N]` | Adjust today's current timed Pomodoro by N five-minute units (`+5` extends by 25 minutes, `-` shortens by 5 minutes; the count defaults to 1); the item must contain only the signed count |
| `++[N]` / `--[N]` | Shift today's running timed Pomodoro N five-minute units later/earlier, keeping its duration (`++3` moves 15 minutes later, `--` moves 5 minutes earlier; the count defaults to 1); the item must contain only the operator |
| `=` / `=<X>` | Start today's next future Pomodoro now with `se<X>` timing (`=` is 25 minutes, `=3` is 15 minutes, `=-2` is 25 minutes with a 10-minute offset), or restart the running session with that timing; a restart prints an override warning; the item must contain only the start token |
| `=name` / `=<X>#pomodoro` | Start the named Pomodoro now with `se<X>` timing (`=bugs` is 25 minutes, `=3#bugs` is 15 minutes); `=bugs` and `=#bugs` are one token — write `#` only when a number comes first (`=3#bugs`) or the name starts with a digit or reads as a close (`=#2fa`, `=#x2`); an open match (whole slug, else prefix) starts in place, a completed match starts a new session with that name ("again"), otherwise a new named session is created and started; while another session runs, a different named target swaps in (an empty `<X>` keeps the running ledger); the item must contain only the start token |
| `=[<X>][#<name>]~<K>` | Start without the queued Task Links in `<K>` (`=~2`, `=3~2,4`, `=#bugs~2`, `=3#bugs~1,3`); `~` drops, the drop part always comes last, and the item must contain only the start token |
| `==` / `==<X>` / `==[<X>]#name` | Retired. For example, `` `==` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=` ``; `==3` gives the same message and teaches `=3`; other claimed forms teach their one-`=` rewrite |
| `x[<N>][*<P>][!<M>][~<K>][@<N>]` | Close today's running timed Pomodoro (lowercase `x` only, with `*`, `!`, `~`, and `@` in any order; `<N>` may instead be typed last as `@<N>`, so `x!1@2` means `x2!1`); `<N>` keeps only those numbered Task Links in progress, `*<P>` parks those links (normal work without carrying forward), `!<M>` completes those links, `~<K>` drops those links (removed, not carried, not started), a lone `0` means none; a present-but-empty `*` or `!` selects every remaining numbered Task Link after explicit assignments and is represented by `park_all`/`complete_all` with an empty lexical list; explicit forms such as `x*1`/`x!1` narrow the action to task 1; one Work Log entry may sit on the close line, several use child bullets |
| `x [<n>] <entry text…>` | One entry on the close line: with no number it logs to the first task the close works (task 1 for plain `x`, the first eligible worked link for wildcard closes); a leading number names the task, so only the first token is an index (`x 1 3 bugs fixed` logs `3 bugs fixed` to task 1) |
| `x` + `- [<n>] <text>` (+ `  - <detail>`) | Several entries while closing: bullets are numbered all or none — a numbered bullet logs its text to worked task `<n>`, while unnumbered bullets log in order to the close's worked tasks (one worked task takes them all, otherwise bullet `i` logs to worked task `i`); a two-space bullet nests an undated detail under its entry; an inline entry plus bullets fails |
| `+2 x`, `x =`, `x =bugs`, `x =~2`, `x wired the lexer =` | Same-line session-operator chain: whitespace-separated session tokens on one line run left to right exactly like blank-line items; an inline entry sits right after its close with operators trailing (`x wired the lexer =` closes with the entry then starts); `x =~2` closes then starts without link 2 of the next lineup, while `x~2 =` drops link 2 of the running session and then starts |
| `x =` + `- 1 wired the lexer` | Close with a Work Log bullet in a chain: the child lines attach to the line's `x`, so this logs the entry then starts the next session |
| `@route:block-id=x…` with no other text | Put that existing task into the running session, then close it (own-prerequisite Blocked targets drop those links first; a refused drop keeps today's history link); the `=` is the suffix separator, the same selection may follow the `x`, and numbers refer to the post-link lineup |
| `^route:block-id=x…` with no other text | Identical execution; `^` is the active-task spelling |
| `<text> @route:block-id=x…` | Create the new Pomodoro-linked task in the running session, then close it; numbers refer to the post-link lineup |
| `@route^block-id+` | Create the project note `<route>_<block_id>.md`; the `^prj` task is never linked and the daily note is untouched |
| `@route^block-id+#pomodoro` | Same; ` :<id>` Task Links go under the named open Pomodoro, creating that named future Pomodoro when missing |
| `- <task> :<task-id>` (project-note bullet) | Name the task, make it Next (`[*]`, or `[?]` when scheduled), and link `[[<stem>#^<task-id>]]` into the current/next Pomodoro, or the `#pomodoro` one |
| `- <task> ^<task-id>` (project-note bullet) | Name the task only: `- [ ] #task <task> [created::DATE] ^<task-id>` |
| `@route+block-id` | Ordinary child bullet under an existing task |
| `@route+block-id#section` | Child bullet under an ALL-CAPS section of that task |
| `@route+block-id` with no other text | Ensure the task is Next and relocate its existing open-Pomodoro Task Link to today's implicit current/next Pomodoro; a task Blocked only by its own prerequisites drops them first |
| `@route+block-id#pomodoro` with no other text | Ensure Next and move that existing Task Link subtree to the named open Pomodoro, or create that named future Pomodoro and move the subtree there; a task Blocked only by its own prerequisites drops them first |
| `@route+block-id!` with no other text | Toggle that task's Pomodoro Task Link: link it when unlinked (Ready and own-prerequisite Blocked rise to Next; other Blocked stay unlinked) or unlink it when linked (lane unchanged) |
| trailing bare `#` | Plain-text note on a Pomodoro (not a routed task) |
| `s:<N>` | `[scheduled::]` N days from today; checkbox-bearing captures start Blocked (`[?]`) |
| `$` / `$<N>` | Write priority level N (bare `$` = 1) and roll a scheduled date in that level's window |
| `%`, `%N`, `%header` | Capture clipboard content as child bullets |

`#` is not one marker. Read it by what it is attached to:

| You typed | Meaning |
| --- | --- |
| `remembered the timeout #` | Pomodoro note |
| `@notes#Ideas` | Bullet under a heading in `notes.md` |
| `@notes#` | Bullet under any non-`Tasks` heading in `notes.md` |
| `@cash+id#requirements` | Child under that task's `REQUIREMENTS` section |
| `@cash+id#coding` with no other text | Ensure that task is Next and move its Task Link to the `CODING` Pomodoro |
| `@sase:deep-fix#bugs` | Pomodoro-linked task under open `BUGS`, creating future `BUGS` when needed |
| `@sase:deep-fix#"SASE V18"`, `=#"DEEP WORK"` | Exact-name literal: the quoted spelling is the name, matched by key equality, never by slug or prefix; straight `"…"` and curly `“…”` both lex, `\"` and `\\` are the only escapes, and the token runs through the closing quote |
| `@sase:deep-fix#bugs=-2` | Same, starting a 25-minute session with a 10-minute offset |
| `^sase:deep-fix#bugs` | Link the existing `^deep-fix` task under open `BUGS`; the `#` names a Pomodoro, and `^` is the active-task spelling |
| `+5` | Extend today's current timed Pomodoro by 25 minutes; the `+` is an adjustment, not a sub-bullet |
| `-2` | Shorten today's current timed Pomodoro by 10 minutes |
| `-` | Shorten today's current timed Pomodoro by 5 minutes; a bare sign is one unit, not an incomplete state |
| `++3` | Shift today's running timed Pomodoro 15 minutes later; the doubled sign moves the whole session |
| `--` | Shift today's running timed Pomodoro 5 minutes earlier; a bare doubled sign is one unit |
| `- foo` | Ordinary task text; a bare sign run followed by more text stays prose |
| `+++` | Ordinary task text; a sign run longer than two stays prose |
| `++3 more` | Error: a shift item must contain only the operator; remove extra text, markers, or child lines |
| `Plan +5` | Ordinary task text; a count mid-body stays prose |
| `x` | Close today's running timed Pomodoro; lowercase `x` is the close token, not task text |
| `x1,3!2` | Close keeping tasks 1 and 3 in progress, completing task 2, deferring the rest |
| `x1~2` | Close keeping task 1 in progress, dropping task 2, deferring the rest |
| `x1!2~3` | Close keeping task 1 in progress, completing task 2, dropping task 3 |
| `x0` | Reset a note-free session to the first future placeholder; close (deferring every numbered Task Link) when the session owns a stand-alone note |
| `x*` | Close parking every numbered Task Link |
| `x!` | Close completing every numbered Task Link |
| `x*1` / `x!1` | Close parking or completing only task 1 |
| `x*!2` | Close parking every link except task 2, and completing task 2 |
| `x1*` | Keep task 1 in progress and park every remaining link |
| `x~` | Incomplete: type a task number after `~` (`capture-parse` needs `pomodoro_close_task`) |
| `=` | Start the next future Pomodoro (25 minutes); a bare `=` is a complete start, not an incomplete state |
| `=3` | Start the next future Pomodoro for 15 minutes; a counted `=` token is a start, not prose |
| `=-2` | Start the next future Pomodoro for 25 minutes with a 10-minute offset |
| `=3 more` | Error: a start item must contain only the token; remove extra text, markers, or child lines |
| `==` | Retired: `` `==` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=` `` |
| `==3` | Retired: `` `==3` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=3` `` |
| `==#bugs` | Retired; Bob removes one leading `=` and prints the corresponding rewrite (`=#bugs`) |
| `=bugs` / `=3#bugs` | While another session runs, swap `BUGS` in and keep the running ledger / start `BUGS` with fresh 15-minute timing (`=bugs` and `=#bugs` are one token) |
| `=bugs` | Start `BUGS` (open whole-slug match, else prefix; completed match starts "again"; otherwise create and start), exactly like `=#bugs` |
| `=bugs@3` | Start `BUGS` for 15 minutes, exactly like `=#bugs@3` and `=3#bugs` |
| `=bugs~2`, `=bugs@3~2` | Start `BUGS` without link 2, with optional timing, exactly like `=#bugs~2` (`@` and `~` in either order) |
| `=bugs more`, `=deep work` | Claimed: the existing named-start shape error with the multi-word hint, in the typed spelling (`=deep-work`) |
| `=bugs=3` | Existing order error, typed spelling: `=bugs@3` or `=3#bugs` |
| `=3bugs`, `=-2bugs`, `=2fa` | New teaching error: the timing runs into a bare name; write `=bugs@3` or `=3#bugs`; a name that starts with a digit takes `#` (`=#2fa`) |
| `@work:outline=bugs` | Link the existing `^outline` task under `BUGS` and start it, exactly like `@work:outline#bugs=` |
| `@work:outline#"SASE V18"` | Same link start under the literally named Pomodoro; the bare `=name` suffix never takes quotes (`@work:outline="SASE V18"` stays prose), so quote only after `#` |
| `@work:outline=bugs@3`, `@work:outline=3#bugs`, `@work:outline=#bugs` | Same link start with timing, under the shared suffix rule; a `~<K>` part in a link suffix (`@work:outline=~2`, `=bugs~2`) gets a teaching error: a link start takes a timing and a name, not a drop list |
| `@work:outline#bugs=focus` | New error: name the Pomodoro once (`@work:outline=focus` or `@work:outline#focus=`) |
| `==important== thing`, `==foo`, `== foo`, `===` | Ordinary task text; `==` is retired, and a bare `==` run followed by prose stays prose, which protects Obsidian highlights |
| `==x` | Error: `` `==x` is not a close: close the running Pomodoro with `x` `` |
| `= foo` | Ordinary task text; a bare `=` followed by prose stays prose |
| `=xx`, `=xa`, `=xmas`, `=Bugs` | Named start (`=xx`/`=xa` were documented prose; that row changes), exactly like `=#xx` |
| `=x`, `=X`, `=x2!1`, `=*`, `=!`, `=x wired the lexer` | Unchanged retired-close teaching errors |
| `=x#bugs` | Unchanged retired error, now teaching `x =bugs` |
| `xx`, `xa`, `x.`, `x-ray`, `xylophone` | Ordinary prose; only a plain `x` or a selection-shaped `x…` token closes |
| `Plan x` | Ordinary task text; a mid-body `x` stays prose |
| `Plan =x` | Ordinary task text; a mid-body `=x` stays prose |
| `X`, `X2`, `X post about the launch` | Ordinary task text; only lowercase `x` closes |
| `X *job:cleanup` | Ordinary blocked-task capture; capitalizing the first letter is the escape for a task that must start with the word `x` |
| `x86 build` | Close claiming task 86 (`x86`); fails unless that Task Link exists |
| `x *job:cleanup` | Error: `` task relationships need a task owner: '*note:block-id' cannot attach to a Pomodoro operator capture (add task text or '@note+task-id' for the dependent) `` |
| `Plan =3` | Ordinary task text; a mid-body `=3` stays prose |
| `x more` | Close with the entry `more` on task 1, exactly like `x` plus `- 1 more` |
| `x2,3 2 foo bar baz` | Close with the entry `foo bar baz` on task 2; `x 1 3 bugs fixed` logs `3 bugs fixed` to task 1 |
| `x 1` | Incomplete: `` `x 1` is incomplete: type the Work Log text after task 1 `` (`capture-parse` needs `pomodoro_close_log_text`); `x1` keeps only task 1 in progress |
| `x3,4` + `- foo bar` + `- baz bam` | Close with `foo bar` on task 3 and `baz bam` on task 4, exactly like `- 3 foo bar` plus `- 4 baz bam` |
| `x` + `- 1 a` + `- b` | Error: bullets are numbered all or none (``start each Work Log bullet with the number of the task it logs to; bullets are numbered all or none: `- 1 b` ``) |
| `x` + `- a` + `- 2 b` | Error: ``Work Log bullets are numbered all or none, and the first one has no task number; to log text that starts with `2`, number every bullet: `- 1 2 b` `` |
| `x3,4` + `- a` + `- b` + `- c` | Error: `` `x3,4` works 2 tasks (3 and 4) but has 3 unnumbered Work Log bullets; start each bullet with the number of the task it logs to: `- 3 c` `` |
| `x` + `- ` | Plain `x`; the placeholder row is ignored |
| `x` + `- 1` | Incomplete: `` `- 1` is incomplete: type the Work Log text after task 1 `` (`capture-parse` needs `pomodoro_close_log_text`) |
| `+2 x` | Extend by 10 minutes, then close; exactly like `+2`, blank line, `x` |
| `x =` | Close the running session, then start the next future Pomodoro |
| `x =bugs` | Close the running session, then start the open `BUGS` Pomodoro; a session switch in one line |
| `=bugs +2` | Swap `BUGS` in, then extend it by 10 minutes; session operators chain left to right exactly like blank-line items |
| `=~2` | Start the next session without queued Task Link 2 |
| `=3~2,4` | Start the next session for 15 minutes without links 2 and 4 |
| `=#bugs~2` | Start the open `BUGS` Pomodoro without its link 2 |
| `=3#bugs~1,3` | Start `BUGS` for 15 minutes without links 1 and 3 |
| `x =~2` | Close the running session, then start the next one without link 2 of that session's own lineup (the links `x` carried into it) |
| `=~2 +2` | Start without link 2, then extend by 10 minutes |
| `=~` | Incomplete: type a task number after `~` (`capture-parse` needs `pomodoro_start_task`) |
| `=~2,` | Incomplete: type a task number after `,` (`capture-parse` needs `pomodoro_start_task`) |
| `=~0` | Error: task numbers start at 1 |
| `=~2,2` | Error: `` task 2 is listed twice in `=~2,2` `` |
| `=~,2` | Error: `` expected a task number before `,` `` |
| `=~2~3` | Error: `` use one `~` list: `=~2,3` `` |
| `=~2!3` | Error: `` a start can only drop Task Links; `!` completes them when you close (`x!3`) `` |
| `=~2#bugs` | Error: `` write the drop list after the name: `=#bugs~2` instead of `=~2#bugs` `` |
| `=~2a` | Error: `` `=~2a` is not a drop list: write `~`, then comma-separated task numbers (for example `=~2,3`) `` |
| `=~ 2` | Error: `` write the task numbers right after `~`, with no spaces (for example `=~2,3`) `` |
| `=#~2` | Error: the existing empty-name error on `=#` |
| `= ~2` | Ordinary task text; a bare `=` followed by separate text stays prose |
| `=deep-work` | Start the open `DEEP WORK` Pomodoro now for 25 minutes; whole-slug match wins, else prefix (`=deep` matches too; `=#deep-work` is the same token) |
| `=#"DEEP WORK"` | Start `DEEP WORK` exactly as spelled: key equality finds the open entry, a completed-only match creates a new session keeping that entry's spelling, otherwise a new session is created with the literal's canonical name; `SASE V18` and `SASE-V18` stay distinct |
| `=#"BOB"` with `BOB BUILD` open | Creates `BOB` instead of selecting `BOB BUILD`: a literal never prefix-matches |
| `=#"DEEP` | Incomplete: type through the closing quote (`capture-parse` needs `pomodoro_name`); an unterminated literal is never a mistake |
| `=3#bugs` | Start the open `BUGS` Pomodoro now for 15 minutes |
| `=bugs@3` | Start the open `BUGS` Pomodoro now for 15 minutes (`=3#bugs` typed last; `=#bugs@3` is the same token) |
| `=bugs@3~2`, `=bugs~2@3` | Start `BUGS` for 15 minutes without task 2 (`=3#bugs~2`; `@` and `~` in either order) |
| `==#bugs@3` | Retired: `` `==#bugs@3` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=#bugs@3` `` (`=3#bugs` is equivalent) |
| `=~2@3` | Start without task 2 for 15 minutes (`=3~2` typed last) |
| `x!1@2` | Close keeping task 2 in progress and completing task 1 (`x2!1` typed last) |
| `x@2` | Close keeping only task 2 in progress (`x2` typed last) |
| `x@` | Incomplete: type a task number after `@` (`capture-parse` needs `pomodoro_close_task`) |
| `x2@3` | Error: lists the in-progress tasks twice (`x2,3` or `x@2,3`) |
| `==#bugs@` | Retired: `` `==#bugs@` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=#bugs@` `` |
| `=3#bugs@5` | Error: gives the timing twice (`=5#bugs` or `=#bugs@5`) |
| `x @2 foo` | Error: `` write the task numbers right after `x`, with no spaces (`x@2`); to log text that starts with `@2`, put the task number first: `x 1 @2 foo` `` |
| `=#` | Incomplete: type a Pomodoro name after `#` (`capture-parse` needs `pomodoro_name`) |
| `=# bugs` | Error: write the name right after `#`, with no space (`=#bugs`) |
| `=deep work` / `=#deep work` | Error: join multi-word Pomodoro names with `-` (`=deep-work`) |
| `=bugs=3` | Error: write the duration after `@` or before the name (`=bugs@3` or `=3#bugs`) |
| `=bugs+2` | Start the Pomodoro named `BUGS+2`; `+` is name charset, so the token stays one named start |
| `x#bugs` | Error: `` `x` always closes the running Pomodoro; remove `#bugs`, or write `x =bugs` to close it and then start that Pomodoro; `=bugs` swaps it in without closing the running session `` |
| `=3#bugs more` | Error: a named start item must contain only the token; remove extra text, markers, or child lines |
| `= -2` vs `=-2` | `= -2` starts 25 minutes then shortens 10 minutes; `=-2` is one 25-minute start with a 10-minute offset |
| `- -` | Two bare adjustments in a row; lines made only of bare operators are chains, not prose |
| `@cash^goog-exit+` | New project note `cash_goog_exit.md`; the `+` is a project-note sigil, not a sub-bullet |
| `@cash^goog-exit+#bugs` | New project note with its ` :id` Task Links under the Pomodoro named `BUGS`; the `#` names a Pomodoro, not a task section |
| `@cash:goog-exit+`, `@cash:goog-exit+#bugs` | Retired project-note forms; use `@cash^goog-exit+` / `@cash^goog-exit+#bugs` and put ` :<id>` on the task bullets instead |
| `@sase:deep-fix#bugs+` | Pomodoro-linked task under the Pomodoro named `BUGS+`; a trailing `+` after a `#name` stays part of the Pomodoro name |
| `Fix it @sase #now` | Error: a trailing `#tag` must be appended to an `@route` token (`@foo#bar`); `#now` was retired |
| `Fix it #now @sase` | Error: a `#tag` before the route is a retired bullet marker, like any other `#tag` |
| `@sase:deep-fix #now` | Error: a trailing `#tag` must be appended to an `@route` token (`@foo#bar`) |
| `Fix it @sase #n` | Error: a trailing `#tag` must be appended to an `@route` token (`@foo#bar`) |

A `#` after `@route+id` follows the item's mode: with no body text it names a
Pomodoro for the task toggle, and once body text is present it names an
ALL-CAPS task section for the child bullet. A `#` in the middle of the body
stays ordinary text. `@route::id` is retired; use `@route^id` for an ordinary
task with a block ID.

Leading `@route text` is accepted only on an item's first physical line. Later
lines in the same item take trailing markers only. `@@...` has no such
restriction: a declaration token may appear on any parent or authored child
line. Terminal `s:<N>`, `$<N>`, `%...`, and `@...` markers configure the whole
item no matter which of its lines they appear on.

## `bob capture`

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures tasks, ordinary Markdown bullets, task sub-bullets, or reference links
without requiring desktop Obsidian to be open. Reference links queue for
background capture as described in [Saving links](#saving-links-to-your-reading-queue).
`TEXT` is one or more physical lines: the first nonblank line is the captured
parent, and whitespace within each line is
normalized, but line breaks are meaningful -- see "Authored sub-bullets"
below for the bounded hierarchy later lines accept. Task mode writes
`- [ ] #task <text> [created::YYYY-MM-DD]` when unscheduled, or `[?]` when a
scheduled property is resolved, and routes to `mac_inbox.md` by default; bullet
mode writes into a selected non-`Tasks` section as described below. The created
date uses the local date from `BOB_NOW`, `DATE`, or the system clock.

### Routing and insertion

Automatic routing uses a leading `@route text` prefix when present; otherwise a
trailing `text @route` suffix is used. A draft-wide `@@route` or
`@@route+block-id` declaration, described below, supplies the same destination
to every item that has no local route or mode marker.
Route names use `A-Z`, `a-z`, `0-9`, `_`, and `-`, are lower-cased, and write
to `<route>.md` at the vault root. Existing target files, including
`mac_inbox.md`, prefer a Markdown `Tasks` section: new captures insert after
the last top-level `#task` block in that section, or after one blank line below
the `Tasks` heading when the section has no tasks yet. When
`bob task reconcile` (formerly `bob task-status-hooks`, still accepted)
has generated a status-count badge row directly below
the heading, the empty-section insertion point is below that row. Files without
a `Tasks` section keep the older fallback of inserting after the last top-level
`#task` block and its indented continuation lines, or appending at EOF.

### Global destination declaration

A global declaration is a whitespace-free `@@<route>` or
`@@<route>+<block-id>` token anywhere in the draft. It may sit at the end of an
item, on an authored child line, in the middle of a line, or on a line by
itself:

```text
Buy milk @@groceries
```

```text
Parent task
- child detail @@work
```

```text
First task

Second task @@foo
```

A physical line whose tokens are all `@@...` declarations is metadata only. It
is removed before blank-line item splitting, so the historical top-of-draft
spelling still behaves the same:

```text
@@foo
First task

Second task
```

```text
@@foo

First task

Second task
```

`@@<route>` sends every otherwise-unrouted item to `<route>.md` as a task.
`@@<route>+<block-id>` inserts every otherwise-unrouted item as its own direct
child beneath that task, in source order; authored children stay nested under
their own capture parent. Route and block-ID validation match `@route` and
`@route+block-id`.

The declaration is metadata, never capture text: it is absent from item counts,
semantic capture text, preview bodies, and note contents. A draft that contains
only a declaration fails with an actionable "add a capture item" error. `@@` is
reserved for this grammar. A second declaration fails with a duplicate global
destination error naming both lines:

```text
@@foo
Buy milk @@bar
```

Unsupported forms such as `@@foo#Ideas`, `@@foo^id`, `@@foo:id`,
`@@foo^id+`, and `@@foo:id+` are errors
rather than literal task text. Wrap `@@...` in inline code to keep it literal.

An item-local marker still wins for that item. If the same item also owns the
declaration, `bob capture-parse` reports a `global_destination_shadowed`
warning and `bob capture` surfaces the same warning:

```text
Buy milk @dev @@groceries

Other task
```

Here `Buy milk` routes to `dev.md`; `Other task` inherits `groceries.md`.

Do not combine a textual `@@` declaration with `--route`, `--section`, `--task`,
or `--task-section`. Clipboard, scheduling, priority, dry-run, formatting, and
stdin remain composable.

Each real item resolves in this order:

1. An item-local route or mode marker wins. That includes `@bar`, `@bar+b-id`,
   `@bar#...`, `@bar^...`, `@bar:...`, `@bar^id+`, and a trailing bare `#`.
2. Otherwise inherit the complete `@@...` declaration.
3. Otherwise keep today's `mac_inbox.md` task default.

A local override is not a duplicate-route diagnostic merely because a declaration
exists. Later items still see earlier staged edits to the same note or parent,
and any failure rolls every target back.

### Scheduling and priority

Append a lowercase `s:<N>` token to schedule the capture `N` days from today.
It is recognized only in the terminal token region and may appear on either
side of a trailing route marker. The token is removed from the body and adds
`[scheduled::YYYY-MM-DD]` after the created stamp. Checkbox-bearing captures
with a resolved scheduled property start Blocked (`[?]`), including `s:0`.
Ordinary bullet and sub-bullet captures still render without a checkbox.
`N` is a non-negative integer. An offset that cannot be added to today
inside the representable calendar — for example `s:9999999999` — fails
before any write. Human output is
`bob capture: scheduled offset is out of range` (exit 2). With
`--format json`, stdout is
`{"ok": false, "error": "scheduled offset is out of range"}`. A `$<N>`
roll that lands outside that calendar uses the same error.

Append a `$` or `$<N>` token to write a priority level, where `N` selects
the Nth level in the bullet-property config file. A bare `$` means `$1`.
Bob looks for that file at
`BOB_CONFIG_FILE`, then `$XDG_CONFIG_HOME/bob/config.yml`, then
`~/.config/bob/config.yml` — the same file the Obsidian picker reads. A missing
or unreadable file is an error; `$<N>` has no built-in default levels. The
currently deployed file uses four levels:

| N | Label | Value    | Day window |
| - | ----- | -------- | ---------- |
| 1 | `P1`  | `high`   | 2-7        |
| 2 | `P2`  | `medium` | 8-30       |
| 3 | `P3`  | `low`    | 31-90      |
| 4 | `P4`  | `lowest` | 91-365     |

The token writes `[priority::<value>]` and rolls a random
`[scheduled::YYYY-MM-DD]` date inside that level's day window. Each capture
rolls independently, so a `--dry-run` preview differs from the real capture
unless `BOB_PRIORITY_ROLL_SEED` is set. A task with no priority field is
implicitly P0 (do it now, no roll), so there is no `$0`. An explicit `s:<N>`
wins the scheduled date; `$<N>` still writes the priority. A rolled `$<N>`
date also writes a `🗓️ **SCHEDULE LOG**` child bullet with one dated `🎲 …`
entry recording why, byte-for-byte matching what the Obsidian
`Ctrl+Shift+P` picker writes for the same level with no reason prompt:

```markdown
- [?] #task someday idea [created::2026-08-07] [priority::lowest] [scheduled::2026-11-06]
	- 🗓️ **SCHEDULE LOG**
		- *2026-11-06* — 🎲 P0 → P4 · in **91** (91–365) days
```

The bold number is the exact relative day offset selected for that scheduled
date. The parenthesized range is the configured priority window.

A `$<N> s:<N>` capture writes no entry, since `s:<N>` wins the scheduled date
and no roll happened. A `$<N>` past the configured level count is a usage
error (exit 2). The message is
`$<N> is not a configured priority level; use $ or $1 through $<count> (<labels>)`.
Any resolved scheduled property makes a checkbox-bearing capture start Blocked
(`[?]`); `bob task reconcile` still reconciles tasks whose schedules are
edited later.

`$`/`$<N>` is recognized only in the terminal marker region, so a mid-line
amount stays text: `Pay $3 to Bob`. In a shell, single-quote the token
(`bob capture buy milk '$2'`); an unquoted or double-quoted `$2` is expanded
by the shell.

To keep a trailing dollar amount as task text, escape it the same way as
`\&` / `\*`: `Pay Bob \$3` writes body `Pay Bob $3` with no priority, and
`Pay Bob \$3 $2 @cash` writes body `Pay Bob $3` at level 2 routed to
`cash`. Elsewhere `\$` stays verbatim (`cost \$3 today` keeps the
backslash).

### Multi-item capture

One or more blank or whitespace-only physical lines split `TEXT` into ordered
capture items. Leading, trailing, and repeated separator runs are ignored, so
a draft with at least one nonempty item is valid even if it starts or ends
with blank rows. Each item uses the normal capture grammar independently:
the first nonblank line is that item's parent, later contiguous authored
bullet rows belong only to that item, and terminal `s:<N>`, `$<N>`,
`%...`, and `@...` markers configure only that item.

Bob plans the whole batch against in-memory note and daily-ledger snapshots
before writing anything. Later items see earlier planned edits to the same
target, so order, insertion points, duplicate block-ID checks, sub-bullet
lookups, and Pomodoro links match a successful sequential capture. If any
item fails to parse, read the clipboard, validate, stage, or replace, Bob
leaves notes, ledgers, and newly-created clipboard files at their original
state. `--dry-run` uses the same planner with the commit step disabled.

Before replacing a note, Bob checks that its on-disk contents still match the
planning snapshot. The check runs before any temporary files are created,
again after those files exist, and once more immediately before each
replacement. A mismatch refuses the batch with exit 1 and one of these
messages:

- `refusing to overwrite <path>: note changed on disk after planning`
- `refusing to overwrite <path>: note was deleted on disk after planning`
- `refusing to overwrite <path>: note was created on disk after planning`

Reload the affected note and rerun the capture. The checks narrow the race
with another writer; they do not lock the file. If a later replacement fails,
Bob restores notes it already replaced in this batch. A successful rollback
appends `; rolled back earlier note writes`. A failed rollback names the
file: `rollback of <path> failed: …`.

Reference jobs are written before note files. If the note write fails, Bob
removes the jobs it just created and appends
`; removed ref jobs created by this capture`. A later clip failure writes a
fallback inbox task for that link and leaves the other captured items in
place.

Same-line session-operator chains split the same way: a physical line holding
only whitespace-separated session operators (`+2 x`) yields one item per
token, in order, exactly like the same operators on blank-line-separated
rows. See [Chaining session operators on one line](#chaining-session-operators-on-one-line).

Single-item success JSON keeps the legacy shape. A multi-item success keeps
the first result in the legacy top-level fields and adds an ordered
`captures` array containing every per-item result. When a `@@` declaration is
present, success JSON also adds an optional top-level `global_destination`
object with `mode`, `route`, and optional `block_id`. The declaration never
creates an extra result. Human output numbers batch items as `1/N`, `2/N`,
and so on, and prints a compact `global  foo.md` (or `global  foo.md · under
^a-id`) summary when a declaration was used.

### Bulk templates

A draft whose first nonblank line is `@@template:` expands one capture item per
row. The planner receives the same text that `bob capture -E` prints, so a
template capture has the same results, ordering, and all-or-nothing writes as
capturing that expansion directly.
Editors that run [`capture-rewrite`](#bob-capture-rewrite), including the Mac
capture panel, turn a typed `@@@` at the very start of a draft into the template
header and place the caret on the next line.

```text
@@template:
Apply to {0}! @job^apply-{1}
- **URL**: {2}

Foobar foo http://foobar.com/career/some_foo_job
"Two Sigma" two-sigma https://www.twosigma.com/careers/some_role
```

This expands to two ordinary items, preserving the authored child bullet:

```text
Apply to Foobar! @job^apply-foo
- **URL**: http://foobar.com/career/some_foo_job

Apply to Two Sigma! @job^apply-two-sigma
- **URL**: https://www.twosigma.com/careers/some_role
```

The template is one item: a parent line and optional authored child bullets.
It ends at the first blank line after the header. Blank lines after that
separate rows and are ignored. Each nonblank row supplies one capture, except
a template using only `{0}` takes one capture per space-separated value, so
values may share a line or sit one per line; rows cannot have child bullets
yet.

| Draft part | Rule |
| --- | --- |
| Header | `@@template:` alone on the first nonblank line; case-insensitive |
| Placeholder | `{0}` through `{999}`; indices must start at zero and have no gaps; repeats and any order are allowed |
| Literal braces | Write `{{` for `{` and `}}` for `}` |
| One placeholder | Split the row on spaces and tabs; each value is its own row, numbered left to right then top to bottom; quote values containing spaces as below |
| Multiple placeholders | Separate fields with spaces or tabs; quote fields containing spaces with `"..."` or `“...”`; `\"` and `\\` are the only escapes inside quotes |

A `{0}`-only template takes one capture per value:

```text
@@template:
foo: {0}! @sase:foo-{0}

bar baz bam
```

This expands to three ordinary items, exactly like one value per line:

```text
foo: bar! @sase:foo-bar

foo: baz! @sase:foo-baz

foo: bam! @sase:foo-bam
```

Values are inserted once and are not scanned again. Every ordinary marker in
the template applies independently to each row, including routes and sections,
`s:<N>`, `$<N>`, `&` and `*` dependencies, completion (`!note:id`), task
links, and reference URLs. Duplicate block IDs across rows are refused by the
normal planner. A row with an empty or incomplete picker value gets the
existing capture error, scoped to that template row.

Use `@route:{1}` when each row should link its task under the one selected
Pomodoro; the normal `plan.max_links` warning applies. Use `@route^apply-{1}`
when rows should create plain Ready tasks that you link as you work them.

Templates refuse other `@@` declarations, clipboard capture, and Pomodoro
session operators. This includes `%` markers even with `--no-clip`, `--clip`,
whole-item forms such as `=`, `x`, `+N`, and `++N`, and start/close suffixes
on task markers. `URL %` remains allowed when it is the reference listen
marker. In a `{0}`-only template, keep the marker on its URL by quoting the
row (`"https://example.com/one %"`) or by putting `%` in the template
(`{0} %` covers the all-listen case); an unquoted
`https://example.com/one %` row is two values and refuses naming row 2. A
template is limited to 1,000 rows and 1 MiB of
expanded text; it is refused rather than truncated when it exceeds either
bound.

Template errors use stable codes in JSON mode. Execution reports the first
problem in draft order and writes nothing:

| Code | Example message |
| --- | --- |
| `template_header_misplaced` | Write `@@template:` alone on the draft's first line |
| `template_missing_rows` | Add a blank line, then one row per capture; with the blank separator present: add one row per capture (`: N values separated by spaces`, `: one value per capture, separated by spaces or new lines`, or bare) |
| `template_needs_separator` | Add a blank line between the template and its rows |
| `template_missing_placeholder` | Add at least one placeholder such as `{0}` |
| `template_invalid_placeholder` | Use `{N}`; write `{{` for a literal brace |
| `template_placeholder_gap` | This template uses `{0}` and `{2}` but not `{1}`; number placeholders without gaps |
| `template_row_bullet` | Rows can't have child bullets yet; put each capture's values on one row |
| `template_unterminated_quote` | Quoted field has no closing quote |
| `template_row_arity` | Template row 3 (line 7) has 4 fields but the template uses `{0}`–`{2}` |
| `template_global_declaration` | A template draft can't declare `@@` destinations; put the destination in the template line |
| `template_clipboard` | A template can't capture the clipboard: `%` would repeat once per row |
| `template_session_operator` | A template can't drive the Pomodoro session; capture `=`, `x`, `+N`, and `++N` on their own |
| `template_too_large` | Expanded template exceeds the 1 MiB limit |
| `template_row_incomplete` | `capture-parse` reports an incomplete expanded item on its row |

Success JSON adds a top-level `template` summary (`line`, `field_count`, and
`row_count`) and a `template_row` (`row`, `line`) on each result item. Human
output prints a dim row-count and source-line summary before the numbered
items. `-E, --expand` prints the expanded text and exits before vault access
or planning; JSON mode returns it as `expanded` with the same `template`
summary. On drafts without the header, `-E` returns the original text.

### Saved templates

Every successful real `@@template:` capture saves its template block to a
bob-owned shelf, so the next wave reuses it instead of retyping it. Only the
block is saved, never the rows: rows carry one-off values that must not
replay.

Saving happens after the vault write succeeds, once per invocation (not once
per row), and only for real captures: `--dry-run`, `-E`, failed or refused
captures, drafts without a header, and forced `-r`/`-s`/`-t` destinations
never touch the shelf. `capture-parse`, `capture-complete`, and
`capture-rewrite` never touch it either. Saving is best-effort and never
fails a capture: the exit code stays 0, and a shelf failure prints
`bob capture: warning: template not saved: <reason>` on stderr with
`template.not_saved` in JSON.

The shelf lives at
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/capture/templates.json`.
`BOB_CAPTURE_TEMPLATES_FILE` overrides the exact file path. The directory is
`0700` and the file is `0600`. Entries are keyed by the first 12 hex digits
of SHA-256 over the canonical block text (trailing spaces and tabs stripped,
lines joined with `\n`), ordered most recently touched first, with
`first_saved_at`, `last_used_at`, and a `use_count` of successful captures.

`capture.max_saved_templates` in the Bob config caps the shelf from 0 to 36
and defaults to 36. Every write trims to the cap by dropping the least
recently used entries. `0` turns saving off: bob never writes the shelf
except for an explicit `delete` or `clear`, and existing entries stay listed
and usable. An invalid value or a config read error prints one stderr warning
and uses 36.

A save may carry the app's checkout origin: when the used block differs from
the checked-out entry, the edited version replaces the original in the same
locked write and keeps its history, so edits never pile up near-duplicates.
Abandoning a checked-out draft leaves the shelf untouched, and the copy on
disk is never deleted by picking it.

Use `bob capture-templates` to manage the shelf. Bare invocation and `list`
show the newest templates first. `show TEMPLATE` prints the ready-to-insert
draft (`@@template:` header, definition, and an empty rows area) without
styling, so it can be piped or copied directly. `TEMPLATE` is a 1-based list
index of one or two digits, or the content id shown by `list`.

```sh
bob capture-templates
bob capture-templates show 1
bob capture-templates save -f json < template-draft.txt
bob capture-templates delete 1 9c1f0e2a7b3d
bob capture-templates clear
```

`save` accepts TEXT arguments joined with spaces or a complete draft on stdin.
It checks the definition (header, separator, placeholders, and block-level
markers) while ignoring missing rows and row errors. Saving a definition does
not increment `use_count`. The hidden `--template-origin ID` supports the Mac
client's checkout flow; it replaces an edited origin atomically while keeping
its history. `delete` is idempotent and reports unresolved selectors in JSON's
`missing` array. `clear` also removes a corrupt quarantine copy and is allowed
when the shelf was written by a newer schema.

`list -f json` returns `schema_version`, `capacity`, `saving`, the resolved
`file`, and entries with their canonical `text`, insertion `draft`, UTF-8 byte
`cursor`, `field_count`, `line_count`, `route`, capture-parse `spans`, and usage
timestamps. The Mac client can render and insert those values without parsing
the capture language. Shelf and config warnings appear in `warnings`.

### Authored sub-bullets

`TEXT` may carry authored bullets beneath the parent line:

```text
Prepare the launch review
- Confirm the rollout owner
  - Send the owner the final date
- Attach the final checklist @work $1
  - Verify the links
```

Within one capture item, every physical line after the first must be a
first-level Markdown item at column zero or a nested Markdown item prefixed
by exactly two ASCII spaces. At either level, `-`, `*`, or `+` must be
followed by at least one space or tab. The source marker and separating
whitespace are stripped, and each item is rendered with the canonical
`- <body>` marker. First-level items render one indentation unit beneath the
captured parent; nested items render two units beneath the parent and attach
to the nearest preceding nonempty first-level authored item. The unit matches
the target note's dominant tab-or-two-space child indentation, with a tab for
a fresh note:

```markdown
- [ ] #task Prepare the launch review [created::2026-08-14] [priority::high]
	- Confirm the rollout owner
		- Send the owner the final date
	- Attach the final checklist
		- Verify the links
```

A marker with nothing after it (`- ` or `  - ` alone) is a harmless
placeholder and produces no child; this keeps interactive editors safe while
a row is only half-typed. Placeholder rows do not clear the current
first-level owner, so a later nested item still attaches to it. A true blank
row ends the item and starts the next item at the following nonblank line.
One-space, three-or-more-space, tabbed, wrapped, or ordinary continuation
prose is a usage error naming the physical line number. A nonempty nested
item before any first-level authored item is an `orphaned_nested_bullet`
error, and an item that becomes empty only because its whole body was a
capture marker is rejected the same way. Every recognized
terminal `s:<N>`, `$<N>`, `%...`, and `@route`/`@route#`/`@route^block-id`/
`@route:block-id`/`@route+block-id`/`@route^block-id+`
marker is an item-wide directive no matter
which physical line in that item it appears on -- as shown above, `@work` and
`$1` on the last child still route and prioritize that item -- and is
stripped from the rendered line it was typed on. A second line in the same
item that resolves the same marker slot (two routes, two schedules, two
priorities, or two clipboard markers) is ambiguous and fails with a usage
error before anything is written. Only the first physical line of an item
keeps the established leading `@route text` form; later lines compose trailing
markers only. A `sub_bullet`
capture (`@route+block-id`) nests the newly captured line under the selected
existing task, then preserves both authored levels relative to that new line.

Authored children render before clipboard children and the priority
schedule log, so the full block order is: parent line, authored children,
clipboard children, then the schedule log.

### Clipboard

Append one of these whitespace-delimited terminal markers to capture clipboard
content beneath the new task or bullet:

- `%` captures the live clipboard once without a header.
- `%<positive integer>` captures exactly that many values without headers: the
  live clipboard first, followed by recent history newest first. For example,
  `bob capture research links %3` captures three values. For ordinary
  non-reference task text, `%1` is equivalent to `%`, leading zeroes are
  accepted, and `%0` stays literal.
- `%<nonnumeric header>` captures the live clipboard once under an explicit
  header. Headers accept letters, digits, `_`, and `-`, render in uppercase,
  and replace underscores with spaces; for example, `%build_log` renders
  `**BUILD LOG:**`.

The grammar is context-dependent for an eligible URL-only reference: exact
standalone `URL %` requests companion audio before clipboard parsing, so it
does not read the clipboard even in live preview's `--no-clip` mode. Use
`URL %1` or `--clip URL` to explicitly request clipboard capture for a URL;
non-reference `text %`, `%N`, and `%header` keep their clipboard meanings.

The marker composes with `s:<N>`, `$<N>`, ordinary routes, bullet routes,
ID-only task routes, and Pomodoro routes in either terminal order. Invalid
`%...` tokens and `%` tokens in the middle of the body stay literal. A counted capture requires every
requested entry to read, normalize, classify, and plan successfully;
insufficient or invalid history aborts the capture instead of writing a
partial result.

Clipboard content is rendered according to its shape:

- One text line up to 1,000 characters becomes an inline child bullet.
- Two to ten flat text lines become child bullets, nested beneath an explicit
  header when one is present.
- One to ten top-level unordered Markdown list items using `-`, `*`, or `+`
  become child bullets. Their source list markers and separating whitespace are
  removed while inline Markdown, including checkbox text, is preserved.
- Absolute file paths (including quoted paths, `file://` URIs, and `~/...`)
  become attachments. Images are copied to `img/` and embedded at 400px;
  other files are copied to `file/` and linked.
- Long, indented, blank-line-separated, or other Markdown-structured text is
  saved verbatim as `file/clip-YYYYMMDD-HHMMSS[-slug].md` and linked without
  the `.md` suffix. Ordered, nested, wrapped, mixed, or empty-item lists use
  this snippet fallback instead of being partially normalized.

Each value in a counted history capture is classified independently, so limits
such as the ten-attachment maximum apply per entry. All resulting lines are
flattened in source order as direct, headerless children; entries receive no
index labels, container bullets, or separators.

Clipboard children use the target note's dominant tab-or-two-space indentation
and fall back to a tab, matching the sub-bullet capture rule.

Without a header, one item is written as a direct child and multiple items are
written as direct sibling children:

```markdown
- [ ] #task Parent
	- clipboard text
- [ ] #task Another parent
	- first line
	- second line
```

For example, a clipboard containing this flat Markdown list:

```markdown
- first copied item
* second item with **inline Markdown**
+ [ ] third checkbox item
```

is normalized beneath the captured parent without doubling the source markers:

```markdown
- [ ] #task Parent
	- first copied item
	- second item with **inline Markdown**
	- [ ] third checkbox item
```

An explicit header stays inline for one item and owns a nested list for
multiple items:

```markdown
- [ ] #task Parent
	- **BUILD LOG:** clipboard text
- [ ] #task Another parent
	- **BUILD LOG:**
		- first line
		- second line
```

Attachment names are sanitized for Obsidian links. An existing identical file
is reused; differing content receives an eight-character SHA-256 suffix. Up to
ten attachment paths may be pasted at once. Clipboard text must be non-empty
UTF-8 without NUL bytes; binary clipboard contents should be represented by a
copied file path. Clipboard and note edits are planned before anything is
written, and newly created clipboard files are removed if the note write fails.
`--dry-run` performs the same planning but creates no directories or files.

Use `-c, --clip[=HEADER]` to force clipboard capture without a marker. Bare
`--clip` captures without a header, while `--clip=build_log` supplies an
explicit header. Both forms force a single live value and keep `%` tokens in
the captured text literal. A numeric header can be requested unambiguously with
`--clip=20`; use `-n, --no-clip` when a genuine trailing `%N` or other `%...`
token should remain literal. `--clip` and `--no-clip` conflict.

### Saving links to your reading queue

A capture item that is nothing but one bare public link becomes a reference
item (`kind: "ref"`) instead of an inbox task. `bob capture
'https://example.com/essay'` saves a background job under `mac_inbox`. Add
exactly one ordinary leading or trailing `@route` (`URL @sase` or
`@sase URL`), a bare URL under a plain `@@sase` declaration, or a bare URL
with `-r sase` to file it under that route instead: aliases resolve to the
canonical route (so `URL @bob-cli` files under `bob`). The body holds only
the URL; the input token and its canonical parent stay distinct, and the
parent source is `explicit`, `global`, or `default`. A new link prints
`queued example.com/essay → sase` with the detail
`new to your library · reading task lands in sase.md` (or `→ mac_inbox` with
`file it later` for the default inbox). That line means the job was saved.
The reading queue in `bob ref list` is the reference note. The background job
files it when it runs; a queued clip appears after the host's intake drain
(`bob ref scan -i`). Quote the URL when the shell would treat `#` as a
comment.

Append a standalone `%` immediately after the URL (the next
whitespace-delimited token on the same line, one or more spaces/tabs) to
request companion audio: `https://example.blog.com/somepost %` clips like
`bob ref create <URL> -P <resolved-parent> -L`. The shipped standalone `@`
spelling remains a compatible alias. `URL % @sase` and `@sase URL %`
combine listen with an explicit parent; `URL %` alone uses the default,
`@@route` global, or `-r` forced parent. Do not attach the directive to the
URL, or put it in a query, fragment, encoded `%25`/`%40`, or userinfo — those
bytes stay part of the URL. One optional `<...>` wrapper around the URL is
still accepted. Exact `URL %` takes reference precedence over clipboard
parsing; `URL %1`, `--clip URL`, and `-R 'URL %'` explicitly keep the
clipboard/task path. Outside an admitted reference shape, `%` keeps its
clipboard meaning, and `--no-clip` leaves ordinary clipboard markers
literal. In live preview, `--no-clip` does not suppress a recognized URL
reference listen marker. A non-URL bare `@` and prefixes such as `@sa`
remain route completion, and completing a claimed listen marker never
replaces it (a separate ` @sa` after `URL %` still completes the parent). A marked item
prints `queued … · Listen` with `· companion audio requested`; it needs a
configured `highlights.listen_command`. Inline intake files the PDF and MP3
together, and a queued capture is paired by `bob ref scan -i` or the local
timer. Unlike an explicit `ref create -L`, the shorthand never
adds audio to existing captures: a marked duplicate, pending job, or
library hit queues nothing and shows `no new listen job was queued · to
attach audio run: bob ref create <URL> -P <parent> -L`.

Anything more — an extra word, `#tag`, `s:`, `$<N>`, a clipboard marker, an operator, a child
line, a `@@route+block-id` declaration, task/destination flags beyond `-r`
(`-s -t -S -c`, `--task-ref`), or a second `@route` — keeps the item a task.
`-R` also keeps it a task. Hosts without a dot (`http://go/x`, `localhost`),
IP literals, and excluded hosts (see the
[URL routing policy](ref.md#url-routing)) stay tasks too. An unresolvable
route fails the item with near-miss hints and the whole batch rolls back; it
never silently creates a new note. A pasted blank-line-free block in which
every line is a bare URL splits into one item per line, and each line is then
classified on its own, so mixed lists queue the links and keep the rest as
tasks. A plain `@@sase` routes default (routeless) refs to `sase` while an
explicit local `@route` keeps local precedence; `@@sase+block-id` keeps the
item a task. Ordinary task `@route` capture does not resolve aliases.

Submit never touches the network and returns at once: it writes a durable
ref job staged with the resolved parent and a detached background worker
clips it through the same engine as `bob ref create`. If the clip fails, the
link falls back to the parent note (`<parent>.md`) with exactly the URL task
line capture previewed, plus a `⚠️` child with the reason and a
`bob ref create <url> -P <parent>` retry command. `bob ref jobs` lists
pending, clipping, and stuck jobs plus finished jobs from the last 7 days
(`--all` shows older finished jobs); see [ref jobs](ref-jobs.md).

`--dry-run` is offline and writes nothing, including no job. Its headline and
one detail line follow the library verdict (with the resolved parent):

| Verdict | Headline | Detail |
| --- | --- | --- |
| `not_found` | `would queue <display> → <parent>` | `new to your library · reading task lands in <parent>.md` (`· file it later` for `mac_inbox`) |
| `legacy` | `would queue <display> → <parent>` | `in your library as a legacy note (<path>) · a fresh copy will be clipped` |
| `unknown` | `would queue <display> → <parent>` | `library check unavailable: <message> · the clip still dedupes` |
| `in_library` | `already in library <path>` | title and reading state, when known |
| `in_intake` | `already queued <path>` | `waiting for bob ref scan` |
| `clipping` | `already clipping <display>` | `a pending ref job has this link · bob ref jobs` |
| `duplicate` | `duplicate <display>` | `same link as an earlier item` |

A real submit uses `queued` in place of `would queue`. The `not_found`
detail becomes `clipping in the background · reading task lands in
<parent>.md` (`· file it later` for `mac_inbox`); legacy and unknown keep the
details above. `placement` is `"queued"` for `not_found`, `legacy`, and
`unknown` (a job will be written) and `"unchanged"` for the other verdicts
(no new job). Reference items report `routed: false`, `route: null`,
`route_label: ""`, and `task_line: ""`, plus an additive `ref` object with
the classified URL, the library verdict, the canonical `parent: {route,
label, kind, source, alias}` (including already-known URLs), the staged
`job`, and the parent `fallback` on queued items. `ref.job` stays `null`
until the commit, so every dry run has `job: null`. Marked references add
`ref.listen: true`; when the URL is already known, `ref.listen_hint` gives
the explicit `bob ref create ... -P ... -L` command because this shorthand
never attaches audio or upgrades an existing job.
`bob capture-parse` reports `ref_parent: {token, source}` on `ref` items
outside `needs` (explicit token, inherited global route, or `mac_inbox`;
source `explicit|global|default`), and a local route token keeps its ordinary
`route` span. An eligible URL listen reference also reports
`ref_listen: true` and a `ref_listen` span over the standalone `%` (or
compatible `@`) token; both additions are omitted when listen is false.
Route completion also matches `project_name_aliases` with
`match_kind: "alias"` after canonical prefix matches.
Opt out per capture with `-R, --no-ref`, or per entry point with
`highlights.url_routing.capture: false` (silent; an invalid config warns and
also keeps the link a task).

### Task with a requested block ID

Use a leading or trailing `@<route>^<block-id>` marker to create an ordinary
open task with a requested Obsidian block ID, without creating or modifying a
Pomodoro task link. For example,
`bob capture '@dev^foobar' 'Some ordinary task.'` writes:

```markdown
- [ ] #task Some ordinary task. [created::2026-07-10] ^foobar
```

The route is lower-cased, and the destination may be an existing note or a
missing note that can be created like any ordinary routed task. The task
remains an ordinary `[ ]` task when unscheduled, or starts `[?]` when scheduled:
priority and scheduled properties render before the final `^block-id`, and the
JSON `kind` stays `"task"`. The block ID uses the same validator as other Bob
task block IDs (letters, digits, and `-`).
Before a real capture or `--dry-run` reports success, Bob rejects an ID that
already appears anywhere in the destination note, leaving the note unchanged.
This form never reads, validates, creates, or writes today's daily note; an
invalid or missing `BOB_DAY_FILE` has no effect. The retired
`@<route>::<block-id>` spelling is no longer accepted; use
`@<route>^<block-id>` instead.

### `#now` is retired

The `#now` weekly-bet tag was retired: the Next lane took over what it
tracked, and Today is read from the ledger at render time (see
`docs/plan.md`). A trailing `#now` is now the ordinary trailing-tag error
like any other `#tag`, and a `#now` in the body stays plain text. Existing
vault lines that still carry `#now` keep working — the text is ignored.

### `p:<N>` is retired

The `p:<N>` priority marker is retired. Write `$<N>` instead (a bare `$`
means `$1`). A typed `p:2` fails with
`` `p:2` is retired; write `$2` for priority level 2 (a bare `$` means `$1`) ``
and capture-parse reports `retired_priority_marker` on that token. A
malformed `p:x` was never a marker and stays ordinary text.

### Pomodoro-linked tasks

Use a leading or trailing `@<route>:<block-id>` marker to create a
Pomodoro-linked next task. For example,
`bob capture '@dev:foobar' 'Some foobar task.'` writes:

```markdown
- [*] #task Some foobar task. [created::2026-07-10] ^foobar
```

It also adds `[[dev#^foobar]]` as a child bullet of an eligible open Pomodoro
in today's daily note. The route is lower-cased and may contain letters,
digits, `_`, and `-`. The block ID may contain letters, digits, and `-`,
the same rule as `@route^id`. Scheduled offsets work in either
terminal order; a scheduled Pomodoro-linked task starts `[?]`, and the block ID
remains the final task token after any `[scheduled::YYYY-MM-DD]` property.
A newly captured task that is Blocked after the gesture is never linked (`docs/task-dependencies.md`
§13.5): a future `s:<N>` (N ≥ 1), open `&` prerequisites, or a blocked
destination project lands the new task `[?]` (plus `#prj_is_blocked` in a
blocked project) with no Task Link. The open prerequisites rise along the
task's edges instead, and JSON reports `link_guard` (`reasons` with the §13.9
kinds, `promoted` with `route#^block` identities, and a one-sentence
`detail`); human output prints the same sentence after the task line.

The daily note is selected from `BOB_DAY_FILE` when set, otherwise from
`<bob-dir>/YYYY/YYYYMMDD.md` using `BOB_NOW` or the local date. Within its
`Pomodoros` section, capture prefers the single open top-level entry with a
recognized bold or legacy time range; when there is no timed entry, it uses the
first open top-level entry. Completed and nested entries are ignored. Multiple
open timed entries are treated as an invariant error for this implicit
selection. The link is inserted after the selected entry's existing children
and reuses their indentation when possible.

An optional `#<pomodoro>` component names the target: `@sase:deep-fix#bugs`
links under the open Pomodoro named `BUGS` and leaves the current Pomodoro
alone. The selector is a slug — ASCII-lowercase, internal whitespace collapsed
to `-` — using `A-Z`, `a-z`, `0-9`, and `& ' ( ) + , . / -`. That is the
task-section selector character set plus `+`. Matching is a whole-slug match in
document order, else the first slug-prefix match, so `#bugs` and `#bug` both
reach `BUGS`, and `#memory` still reaches `MEMORY` when `MEMORY WORK` appears
first. Duplicate names resolve to the first open match; give the second a
distinct name to target it. A typed `#` with an empty name is incomplete — it
never falls back to "any Pomodoro". An existing open match is explicit, so it
resolves even when the ledger has more than one open timed entry. Quote the
name after `#` (`@sase:deep-fix#"SASE V18"`, with straight or curly quotes) to
spell it exactly: the literal matches by key equality, never by slug or
prefix, so `SASE V18` and `SASE-V18` stay distinct and `#"BOB"` creates `BOB`
even with `BOB BUILD` open. An unterminated literal (`@sase:deep-fix#"SASE V`)
is incomplete, never a mistake.

When the selector has no matching open entry, capture treats the authored
component as the name of a new future Pomodoro. The visible name uses the same
canonicalization as `bob capture-pomodoro-name`: whitespace is collapsed,
ASCII letters are uppercased, and allowed punctuation is preserved, so
`#after-tui-fix` creates `AFTER-TUI-FIX`. The new entry renders as
`- [ ] () — NAME` followed by the captured task link as its child. It is
inserted after the current Pomodoro's complete block when there is one,
otherwise after the last completed Pomodoro's complete block, otherwise before
the first Pomodoro in the section. Completed-only matches create a new future
Pomodoro keeping the completed entry's spelling — reusing completed
`deep-work` creates `DEEP WORK`, never `DEEP-WORK`; completed history is not
modified.
Cancelled, nested, and fenced lookalikes are not anchors. Multiple open timed
entries remain an invariant error when a new named entry would need the
current-Pomodoro anchor.

The routed note and daily note are both parsed and validated before either is
replaced. A missing daily note or Pomodoros section, no eligible unnamed target,
timed ambiguity for implicit or named-creation captures, invalid Pomodoro name,
malformed marker, duplicate block ID, or duplicate Pomodoro link leaves both
notes unchanged. `--dry-run` and multi-item batches use the same staged
daily-note snapshot as a real capture, so a later batch item can reuse a
Pomodoro created by an earlier item without creating a duplicate.

### Starting the session atomically

Append `=<X>` to a `@<route>:<block-id>[#<pomodoro>]` marker to start the
selected Pomodoro in the same transaction: `bob capture 'Write outline
@sase:outline=3'` creates the task, links it to today's next session, and
starts a 15-minute session. `@sase:outline#deep-work=-2` uses the named
target and starts a default 25-minute session with a 10-minute offset.
Prefer the short name spelling: `bob capture '@sase:deep-fix=bugs'` links
under `BUGS` and starts it, exactly like `@sase:deep-fix#bugs=`.

`<X>` is exactly the suffix of the Obsidian bob-ledger-tools `se<X>`
snippet:

| Suffix | Duration | Offset |
| --- | --- | --- |
| (empty) | 25 minutes | none |
| `3` | 3 × 5 minutes | none |
| `-` | 25 minutes | 1 × 5 minutes |
| `-2` | 25 minutes | 2 × 5 minutes |
| `3-` | 3 × 5 minutes | 1 × 5 minutes |
| `2-1` | 2 × 5 minutes | 1 × 5 minutes |

An omitted duration means five 5-minute units (25 minutes); an omitted
offset with `-` means one unit; no `-` means zero offset. `0` and leading
zeros are valid. At the capture clock's local hour/minute, `start =
ceil((nowMinutes - 5*offsetUnits)/5)*5` and `end = start +
5*durationUnits`, displayed modulo 24 hours as `(**HHMM-HHMM** [t:: Nm])`
byte-for-byte like the snippet's range. `BOB_NOW`/Bob's existing clock sets
the time; numeric overflow is a useful error rather than a wrong time.

The suffix is item-local and applies only to new `:` task captures — not
`+` ensures, project-note `+` forms, global `@@` declarations, or ordinary
text. It cannot combine with `s:<N>` or `$<N>`: a scheduled Blocked task
cannot start its session, so those combinations fail before anything is
written. Only an untimed, open placeholder is ever started. Any open timed
Pomodoro — including one past its nominal end — is restarted by a whole-item
`=` start, with a visible warning before the write. Link starts keep the
existing "finish the current Pomodoro first" error because they never override
a running entry; only `x` closes one. A selected non-placeholder or structurally
ambiguous ledger fails with the same error. A newly created started entry uses the
same placement as named creation: after the last completed Pomodoro's complete
block, otherwise before the first Pomodoro in the section. Starting an existing
placeholder moves it, with its child block, to that same slot. JSON output keeps
every existing key and adds an additive `pomodoro_start` object (resolved
`start`/`end`, `duration_minutes`, `offset_units`, destination name/line,
whether the entry was created); human output names the Pomodoro and its time.

### Linking and starting existing tasks

Capture a whole item that is only `@route:block-id[#pomodoro][=<X>]` (or the
`^route:block-id[#pomodoro][=<X>]` active-task spelling, including the short
`@route:block-id=name` / `^route:block-id=name` suffix) to link an existing
task into today's Pomodoro ledger and optionally start that session
atomically. `bob ref today` reuses this planner for a reference's reading
task; its exact-name (`-e`) and entry-ref (`-P`) targets are
`bob ref today`-only and the capture grammar is unchanged. For example, `bob capture '^sase:deep-fix='` links the existing
`^deep-fix` task in `sase.md` and starts its session, and
`bob capture '@sase:deep-fix=bugs'` links it under `BUGS` and starts that session. The marker must be the
entire capture item: no body text, no authored child bullets, no `%`/`--clip`,
no `s:<N>`/`$<N>`, and no forced destination flags. A token with the complete
`^route:block-id…` shape claims its item; anything else on the item fails with
an `invalid_pomodoro_link` error instead of becoming task text. Partial shapes
(`^`, `^fragment`, `^route:`, `^route:block-id#`) are incomplete editing
states when they are the whole item and ordinary prose otherwise. Anything
else starting with `^` (`^_^`, `^^`, `^.`) stays ordinary text, and `^` is
only recognized as the first token of an item's first line. `^…+` and `^…!`
are rejected with messages naming the `@route^block-id+` project-note and
`@route+block-id!` toggle spellings. A `@@` declaration never applies to a
`^` item. Body-bearing `<text> @route:block-id[#name][=<X>]` keeps its exact
current new-task meaning.

The existing task resolves with the task-toggle lookup, so missing notes,
missing IDs (with a close-match suggestion plus a hint to add text for a new
Pomodoro-linked task), duplicate IDs, and non-task block IDs reuse those
errors. Ready `[ ]` and Blocked `[?]` tasks become Next `[*]`; Next stays
Next and In Progress `[/]` is never demoted. A single valid future
`[scheduled::]` is retired with the pull-forward Schedule Log entry when a log
already exists — all under the existing Ensure Next rules, plus the §13.5
link guard: a target that is Blocked after the gesture stays unlinked while
its open prerequisites (own plus inherited) rise, unless §13.5.1 permits
capture to drop its own prerequisites and link in the same staged write.
Project-blocked tasks and `^prj` targets keep their prerequisites and stay
unlinked. A refused drop adds `drop_refused` to `link_guard`. The one refusal is a `^prj` whose only reason is its own future
schedule: it fails with "move the project date from the `^prj` Schedule
action" instead of pulling the date forward. Done, canceled, and unknown
statuses fail: only Ready, Blocked, Next, and In Progress tasks can be linked.

Let _Q_ be the open Pomodoro whose children hold the task's dedicated
`[[route#^id]]` Task Link (links under completed Pomodoros are history and are
never edited; more than one movable link is the existing write-free invariant
error). With `#pomodoro`, the existing named selection applies: whole-slug
then prefix over open entries, else the canonical named future Pomodoro is
created, and _Q_'s complete subtree moves there (or a new link is inserted
when there is no _Q_). Without a name, an already-queued task respects the
queue: the destination is _Q_ and nothing moves. Without a name and no _Q_,
the implicit current/next Pomodoro is used, exactly like a new-task capture.
`=<X>` uses the same `se<X>` math, clock, and guards: any open timed Pomodoro
fails with "finish the current Pomodoro first"; `#name=<X>` needs an untimed
open placeholder or creates a started entry; no-name with _Q_ starts _Q_
(it must be an untimed open placeholder); no-name without _Q_ uses the
first open untimed placeholder or creates an unnamed started entry. Every
started entry moves, with its child block, to the current slot: right after
the last completed Pomodoro's complete block, otherwise before the first
other open Pomodoro.

With `BOB_NOW=2026-07-10 09:02:00` and a ledger holding completed `PLAN`,
open `BUGS` (with `[[sase#^deep-fix]]` plus child notes), and an open unnamed
entry, while `sase.md` holds `[*] ^deep-fix`, `[/] ^outline`, and `[ ]
^ready`:

| Item                     | Result                                                        |
| ------------------------ | ------------------------------------------------------------- |
| `^sase:deep-fix`         | True no-op: already Next, Task Link already in BUGS           |
| `^sase:deep-fix=`        | BUGS becomes `(**0905-0930** [t:: 25m]) — BUGS`; link kept    |
| `^sase:deep-fix#focus=3` | Started `FOCUS` (0905-0920) created after PLAN; link moved    |
| `^sase:outline=`         | Stays `[/]`; BUGS starts with a new `[[sase#^outline]]`       |
| `@sase:ready`            | `[ ]` → `[*]`; new link under BUGS                           |
| `@sase:ready#bugs=-`     | `[ ]` → `[*]`; BUGS starts 0900-0925; link appended           |

Successful JSON reports kind `"pomodoro_link"` with `text: ""`, the post-image
`task_line`, `placement: "linked"`, the task-toggle vocabulary (`block_id`,
`day_file`, `block_link`, `previous_task_line`, `previous_status_symbol` /
`_name`, `status_symbol` / `_name`, `status_changed`, `removed_scheduled`,
`schedule_log` when written, `pomodoro_name`, `creates_pomodoro`),
`pomodoro_link_action` (`"linked"`, `"moved"`, or `"already_current"`),
pre-image `pomodoro_link_source` (only when a link existed), post-image
`pomodoro_link_destination`, `pomodoro_link_placement` (when a link was
inserted or moved), and the exact `pomodoro_start` object `pomodoro_task`
uses (only with `=<X>`). It emits no `toggle_direction`/`toggle_behavior`.
When the guard consumes the gesture, JSON uses the additive kind
`"link_guard"`, omits `block_link`, `creates_pomodoro`,
`pomodoro_link_action` and its endpoints, `pomodoro_link_placement`, and
`pomodoro_name`, and reports `link_guard` (`reasons`, `promoted`, `detail`).
When existing live links are removed, top-level `removed_pomodoro_links` and
`day_file` report the cleanup; neither field is copied into the nested
`link_guard` object. With no cleanup, both fields are omitted. This neutral
kind lets older clients show a generic preview instead of a false link
confirmation; newer clients can render the guard details. Any live links
removed from today's open Pomodoros appear in the batch-level `pomodoro_blocks`
diff. Human output says `not linked` and reports the guard detail without a
generic capture confirmation. Successful human output follows the Ensure Next
style: a `link`/`would link` or
`start`/`would start` header, a status line (`[ ] → [*]`, `[*] already
Next`, `[/] stays In Progress`), a ledger line (`Linked … under BUGS`,
`Moved Task Link BUGS → FOCUS (created FOCUS)`, `Task Link already in
BUGS; no ledger change.`, or the `⛔ not linked (…) · promoted …` guard
sentence), and the existing start phrasing.

### Picking any open task with ':'

Type `:` at the very start of an input to fuzzy-search linkable open tasks
(Ready `[ ]`, Blocked `[?]`, Next `[*]`, or In Progress `[/]`) in the
routable inbox, area, and non-terminal project notes listed by
`bob capture-targets`. Nested notes, reference notes, untyped notes other than
the inbox, and terminal projects are outside this catalog. An ID-less row
requires the explicit Add block ID action before it can be accepted.
Accepting an identified row replaces the `:` query with the
canonical `@route:block-id` link, which captures exactly like a typed link:
as-is it links the task, and with a trailing `=` it also starts that
session. Acceptance edits the draft; submitting it performs the vault write.
A `:` query is only ever a picker query — it is never executable
and never a third spelling of the link.

An item is a task-link query exactly when it has one physical line and that
line (ignoring leading and trailing whitespace) is a single
whitespace-delimited token starting with `:`. The token is the sigil plus a
query of any non-whitespace characters, possibly empty. Each item of a
blank-line-separated batch draft is judged alone, so bulk linking works, and
a `@@` declaration never applies to a query item.

| Draft item | Reading |
| --- | --- |
| `:` | Query `""`: the picker opens on the full list |
| `:dee` | Query `dee` |
| `:sase:deep-fix` | Query; execution teaches the `@` spelling (T2 below) |
| `:sase:deep-fix=` | Query; execution teaches the `@` spelling (T2 below) |
| `: dee`, `:dee more`, `Buy :dee` | Prose: several tokens, unchanged |
| `:dee` with a child line | Prose: several lines, unchanged |

`bob capture` (and `--dry-run`) rejects a claimed item before any other item
parser, so the whole batch rolls back and forced flags change nothing. The
teaching errors keep the usual `capture item K starting on line L: …`
prefix. T1 reads `` `:dee` opens the task picker; pick a task to insert its
`@<route>:<block-id>` link, or write the link yourself (for example
`@sase:deep-fix`) ``. T2 applies when `^` plus the query parses as a
complete caret link, and reads `` `:sase:deep-fix=` opens the task picker;
to link that task, write `@sase:deep-fix=` ``. A finished query can never
silently become a junk inbox task.

Only statuses the link path accepts are listed: Ready `[ ]`, Blocked `[?]`,
Next `[*]`, and In Progress `[/]`. Notes are the `capture-targets` set: the
inbox (when the file exists), area notes A→Z, then non-terminal project
notes A→Z. Done, canceled, and unknown statuses are excluded, as are notes
in terminal projects and untyped vault-root files.

Groups, in precedence: `queued` (an identified task whose dedicated
`[[route#^id]]` link sits under an open Pomodoro today), `in_progress`,
`next`, and `note` (everything else). The empty query keeps the canonical
order: `queued` in ledger order, then `in_progress` and `next` each by
route then line, then `note` tasks in targets order and document order.
Ranking splits the query into lowercase
whitespace-separated terms that must all match over `route:block-id`, the
block ID, the text, the route, the section, and the Pomodoro name — field
prefix first, then word prefix, substring, and in-order subsequence — with
ties keeping the canonical order.

Tasks without a block ID always list, with `requires_block_id: true`, an
empty `replacement` clients must never insert, and up to 3 deterministic
`block_id_suggestions` that are free in that note (for example
`Fix flaky gkeep test` suggests `fix-flaky-gkeep`). Name one with
`bob capture-task-id --route sase --task-ref <ref> --block-id fix-flaky-gkeep`
using the candidate's `route` and `ref`, then capture the link with an
optional `=` as usual. Linking retires a single future
`[scheduled::YYYY-MM-DD]` exactly like a typed link; candidates flag that
with `pulls_forward: true` and the `scheduled` date.

### Going to a task with '/'

Type `/` as the whole capture draft to fuzzy-search the same catalog `:` uses:
linkable open tasks (Ready `[ ]`, Blocked `[?]`, Next `[*]`, or In Progress
`[/]`) in the routable inbox, area, and non-terminal project notes listed by
`bob capture-targets`, as described in [Picking any open
task](#picking-any-open-task-with). Picking a row opens that task in Obsidian
at its home note — caret at column 0, line centred — instead of inserting a
link. Go To never writes: no block-ID minting, no freshness stamps, no lane
changes, no ledger edits, no walk moves, no Task Card opens, no completions.

A draft is a Go To query exactly when the whole draft is one
whitespace-delimited token starting with `/` and holding no second `/`. The
token is the sigil plus a query of any non-whitespace characters, possibly
empty. Unlike `:`, the claim is per draft, not per item.

| Draft | Reading |
| --- | --- |
| `/` | Query `""`: the picker opens on the full list |
| `/fix` | Query `fix` |
| `/sase:deep-fix` | Query `sase:deep-fix` (a `:` inside the query is fine) |
| `  /fix\n` | Query `fix` (surrounding whitespace is ignored) |
| `//`, `/Users/x`, `/r/rust` | Prose: a second `/` |
| `/a b`, `Buy /x` | Prose: several tokens |
| `/x` with a child line | Prose: several lines |
| `/x`, a blank line, then `Buy milk` | Prose: two items, both captured as usual |
| `@@work` on one line, then `/x` | Prose: `/x` routes to `work` as usual |

`bob capture` (and `--dry-run`) refuses a claimed draft before any other item
parser — before the `&` dependency scan, and so before the `:` refusal — so
the whole batch rolls back and forced flags such as `-r` and `-s` change
nothing. No vault file changes. The teaching error (T3) keeps the usual
`capture item 1 starting on line L: …` prefix and reads `` `/dee` opens the
Go To picker; pick a task to open it in Obsidian — it never captures `` (the
quoted token is the exact typed token, so a bare `/` reads `` `/` opens …
``).

`bob capture-parse` reports a claimed draft as mode `incomplete` needing
`task_goto`, with one `interactive_placeholder` span over the whole token,
sigil included, and no diagnostics. `bob capture-complete` serves context
`task_goto`: the same rows and order as `task_link` for the same query,
including ID-less rows (every row opens directly) and the inbox. Each
candidate carries `replacement` (always `""`; clients never insert it),
`ref` (`<line>:<digest>`), `route`, `note_kind`, `note_path` (the exact
vault-relative path with extension), `target` (the absolute path,
`bob_dir.join(note_path)`), nullable `block_id`, `status_symbol`,
`status_name`, `status_type`, `text`, nullable `section`, `depth`, 1-based
`line`, `group`, nullable `scheduled` (context only), nullable `pomodoro`,
and `task_kind` (omitted when absent). There is no `requires_block_id`,
`block_id_suggestions`, or `pulls_forward`. The query is the token text
between the sigil and the cursor (empty at or just after the sigil); the
replacement covers the whole token, sigil included, so refetching at
`replacement.start` returns the unfiltered list. The response carries no
`query` field and no `picker` descriptor, exactly like `task_link`. Human
rows read `[status] text  · note_path:line`, with `· scheduled YYYY-MM-DD`
whenever the task carries a scheduled date; the shell format yields no rows,
because nothing is insertable. The navigation itself runs through
bob-navigation-hotkeys' `obsidian://bob-goto-task` URI, whose contract is in
the [Go to task URI reference](reference/plugins/goto-task-uri.md).

### Choosing a parent task with `+`

Type `+` at the start of an otherwise empty item, or after a space at the end
of a capture line, to search the same task catalog as the `:` picker. Typing
letters before the picker response arrives seeds the search, so `+bank` finds
matching tasks. A terminal `+` in prose such as `Call the bank +` appends the
selected task marker to that line; a leading `+` in a fresh item produces the
marker by itself. The selector is unresolved input: `bob capture` refuses it
until an editor replaces the token with an identified `@route+block-id` marker.

The picker searches open Ready, Blocked, Next, and In Progress tasks in the
routable inbox, area notes, and non-terminal project notes. It uses the full
capture-target catalog across notes; it does not search arbitrary nested or
unroutable notes, or closed tasks. Results keep the `:` picker order and
ranking, including ID-less tasks. An ID-less row has no insertable replacement;
accepting it explicitly opens the stale-safe Add block ID flow. After a
successful `capture-task-id --route` assignment Bob returns a
`parent_replacement` such as `@sase+fix-flaky-gkeep` for the editor to insert.
Choosing an identified row only prepares the draft: a body-bearing marker
appends a note under the task, while a solo marker keeps the existing Ensure
Next behavior.

The exact lone `+` is dual-use. Editors offer the task picker and show the
Escape hint to extend the running Pomodoro by five minutes. Escape preserves
the draft, so submitting `+` still performs that existing adjustment. Numeric
adjustments (`+N`), double-plus shifts (`++N`), and numeric near misses remain
Pomodoro operators. A prose-terminal `+` is always a task selector. Shell
completion lists only identified `@route+block-id` rows and never assigns an
ID or writes the vault.

### Adding prerequisites with '&'

`&note:block-id` means "the task I am capturing or explicitly targeting
depends on the task at `note.md` / `^block-id`". It adds a plain task
dependency link (normally `[[note#^block-id]]`) to the dependent's managed
`⛓️ **DEPENDS ON:**` child line (see `docs/task-dependencies.md`). The note
is a vault-relative identity, not a capture destination: simple root notes
use `&foo:bar`, nested notes use `&projects/foo:bar`, and notes with
whitespace or reserved punctuation use a quoted component such as
`&"Shopping List":bar` (inside quotes only `\"` and `\\` are escapes; case
and Unicode are preserved). Block IDs keep Bob's existing rules. The note
component is extensionless, like current routes; an explicit relative path
resolves first, otherwise a unique basename. Absolute paths, empty
components, and `.`/`..` are rejected.

Modifiers sit at the leading or trailing marker positions of the parent
line, interleaving with destination, schedule, priority, and clipboard
markers in either order, or close an authored child line (a child holding
only modifiers contributes no empty bullet). Both runs skip the same
trailing-transparent marker shapes (`@…`, `#…`, `s:`, `$`/`$<N>`, schedule,
priority, and clip tokens), so `@cash &foo:bar Buy milk` is a dependency
on a `cash.md` task, not literal text. An explicit `@route+task-id`
selects the existing parent even with child-bullet text; a bare solo
`@route:task-id` alongside modifiers is a narrow alias for the same owner.
Without `&`, both spellings keep their existing link/toggle meanings.

| Draft item | Reading |
| --- | --- |
| `Buy Groceries! &foo:bar` | New inbox task depending on `foo.md#^bar` (`task` + `new_task` target) |
| `Buy Groceries! @home &foo:bar &cash:budget` | New `home.md` task with two prerequisites, in typed order |
| `@cash &foo:bar Buy milk` | New `cash.md` task depending on `foo.md#^bar` (leading run skips the destination marker) |
| `&foo:bar @body+excercise` | Add the dependency to existing `body.md#^excercise` (`task_dependency`, no new task) |
| `&foo:bar ^body:excercise` | Same owner via the bare `^` alias; no Task Link is written |
| `&foo:bar @body:excercise` | Same owner via the narrow colon alias (only when bare: no `#name`, `=…`, or `!`) |
| `Waiting for approval @body+excercise &foo:bar` | Keep the sub-bullet prose, add the dependency to its explicit parent |
| `&foo:bar` | Incomplete: needs `dependency_target` (type task text or `+` to choose an existing owner) |
| `&foo:bar +` | Incomplete: needs `task_parent`; the picker chooses the task that will depend on `foo.md#^bar` |
| `&`, `Buy Groceries! &fo`, `Buy Groceries! &foo:` | Incomplete: needs `task_dependency` (the picker stays open) |
| `Research & Development`, `R&D research` | Literal text: mid-word and between-words `&` is never a directive |
| `Buy Groceries! \&foo:bar` | Literal task text `Buy Groceries! &foo:bar` (only the `\` is consumed) |
| `&foo:bar` inside `[[…]]` or `` `…` `` | Literal text: wikilinks and code are never scanned |

`capture-parse` reports each modifier in per-item `dependencies` (`raw`,
decoded `note` and `block_id`, sigil-inclusive `range`) with
`dependency_sigil`, `dependency_note`, and `dependency_block_id` spans, plus
`dependency_target` (`new_task` or `existing_task` ownership). A complete
dependency-only action reports mode `task_dependency`; new-task and
sub-bullet captures keep their mode and gain the modifier fields. Frozen
v1 examples (schema version 1, additive fields only):

```json
// bob capture-parse -f json -- 'Buy Groceries! @home &foo:bar &cash:budget'
{
  "body": "Buy Groceries!",
  "mode": "task",
  "route": "home",
  "needs": [],
  "dependencies": [
    {
      "raw": "&foo:bar",
      "note": "foo",
      "block_id": "bar",
      "quoted": false,
      "range": { "start": 21, "end": 29 }
    },
    {
      "raw": "&cash:budget",
      "note": "cash",
      "block_id": "budget",
      "quoted": false,
      "range": { "start": 30, "end": 42 }
    }
  ],
  "dependency_target": { "kind": "new_task", "route": "home" }
}
```

```json
// bob capture-parse -f json -- '&foo:bar @body+excercise'
{
  "body": "",
  "mode": "task_dependency",
  "route": "body",
  "block_id": "excercise",
  "needs": [],
  "dependencies": [
    {
      "raw": "&foo:bar",
      "note": "foo",
      "block_id": "bar",
      "quoted": false,
      "range": { "start": 0, "end": 8 }
    }
  ],
  "dependency_target": {
    "kind": "existing_task",
    "route": "body",
    "block_id": "excercise"
  }
}
```

`capture-complete` serves context `task_dependency` with a replacement range
covering exactly the active modifier (sigil and quoted note included), the
decoded `query`, the lexical `owner`, and the vault-wide candidate list:

```json
// bob capture-complete -b VAULT -c 19 -f json -- 'Buy Groceries! &foo'
{
  "cursor": 19,
  "replacement": { "start": 15, "end": 19 },
  "context": "task_dependency",
  "candidates": [],
  "query": "foo",
  "owner": { "kind": "new_task", "text": "Buy Groceries!" }
}
```

The discovery scan lists every task-bearing vault note — untyped root
notes, nested folders, ref notes, terminal projects, daily notes, hidden
tasks, and completed/cancelled/archive history — excluding only
dot-directories, `_templates`, `_generated`, `_conflicts`, fenced code, and
non-task block anchors. It never requires today's ledger file. Empty-query
order is same-note open tasks, In Progress, Next, other open tasks grouped
by note, then completed/cancelled history, with hidden tasks last within
their section; queries rank with the same tiered AND-term matcher as the
`:` picker (task text, exact note locator, block ID, path, section) and
keep completed history findable behind open matches. Each row carries the
exact `note_path` (with extension, never a lowercased route), display
`locator`, stable `group`, `already_dependency`, and optional
`disabled_reason`; ID-less rows carry `requires_block_id` with suggestions
and no insertable replacement, and self/already-selected/duplicate-ID rows
stay visible with their guard reason. Refetching at `replacement.start`
returns the unfiltered snapshot.

Contract status: parsing, spans, needs, completion ranges, the candidate
wire shape, and the discovery scan are frozen above, and `bob capture`
executes the modifiers through the staged batch writer: the dependent
gains (or merges into) the managed `⛓️ **DEPENDS ON:**` child line plus
the derived `[dependsOn:: …]` field, open prerequisites flip it to
`[?]` Blocked, existing dependents are freshness-stamped, and repeats
are idempotent (`0 added`). Each added prerequisite's `🧱 **BLOCKS:**`
line is staged in the same batch (the dependent's minted `^id` when it
has none), reported as `blocks_line_updates` plus
`minted_dependent_block_id` when a mint happens; the prerequisite is
reported as a `dependency_target` task block
([task-dependencies.md §14](task-dependencies.md#14-blocks-lines)).
Multi-note batches plan every note before writing anything; a failed
preimage check or write aborts the batch and rolls back notes already
staged, and dry runs plan through the same planner without touching the
vault. A draft with prerequisites never
captures without them. Section bullets, project/note constructions,
Pomodoro ledger links, and session operators reject the modifiers with
a targeted diagnostic, as do suffixed (`#name`, `=…`) or `!`-toggled
dependency-only owners; missing notes, missing block IDs,
self-dependencies, and batch cycles fail with an actionable error and
leave the vault intact.

`&` aimed at a project's `^prj` writes the same Depends-On line and
field, plus the project side effects (`docs/task-dependencies.md`
§13.10): targets inside the same project note refuse (a project cannot
wait on its own tasks), while pre-existing hand-authored same-note
links stay on the line unprojected. When any target is open, `^prj`
and every open resident flip to `[?]` in the same write, residents
gain exactly one `#prj_is_blocked`, and their live links leave
today's open Pomodoros. Open
prerequisites rise to at least the strongest lane among `^prj` and its
residents — never onto a `^prj` target, which the walk passes through
to that project's own prerequisites. More generally, any `&` that
leaves its dependent Blocked consumes that dependent's live links in
the same write. Capture consumption is one shared path for `&`, `*`,
and the link-guard Consume branches: match by identity, keep authored
notes under a consumed link-only bullet, delete only Pomodoro entries
this capture emptied, and report `consumed_links[]` plus `removed`
Pomodoro blocks ([Blocking existing tasks with '*'](#blocking-existing-tasks-with);
[task-dependencies.md §13.5](task-dependencies.md#135-consumption),
PDL8–PDL11). Reconcile and plugins keep removing the whole subtree
(PDL7). Human output prints the structured `−` consumption lines and
suppresses the legacy `removed N live Pomodoro link(s) to newly
Blocked task(s)` warning string, which still appears in `warnings[]`
for older clients.

A successful `&` item always carries `dependency_update` and
`dependency_updates[]` (the array's first entry equals the singular
field, with `role: "dependent"`). See
[Dependency updates and consumed links](#dependency-updates-and-consumed-links).

### Blocking existing tasks with '*'

Obsidian's existing-task twin is the reverse Blocks stage, documented in
[task-dependencies.md §15](task-dependencies.md#15-the-blocks-stage-navigation-hotkeys).

`*note:block-id` is the dual of `&note:block-id`. Let N be the item's
task (new, or an existing owner) and T the task named with `*`. Arrows
mean "depends on":

| Draft | Stored edge | Line that changes |
| --- | --- | --- |
| `N &P` (today) | N → P | N's Depends-On |
| `N *T` | T → N | T's Depends-On |
| `N &P *T` | T → N → P | both ("insert between") |
| `*T @route+id`, `@route:id *T` | T → `route#^id` | T's Depends-On |
| `Note text @route+id *T` | T → `route#^id`, plus the sub-bullet | T's Depends-On |

The edge lives only on T's `⛓️ **DEPENDS ON:**` line, and N's
`🧱 **BLOCKS:**` line is the usual generated mirror. No new data model,
field, or tag is added. `*` shares `&`'s ownership rules
(`resolve_execution_ownership`, `@@` inheritance, forced
`--task`/`--route`), child-line trailing runs, and rejections.
Quote `*` on the shell, because zsh globs it:
`bob capture 'Ask Kelly *job:cleanup'`.

#### Grammar

`*` is a directive only when **all** of these hold:

1. **Placement.** It starts a whitespace-delimited token in the parent
   line's leading run or in any line's trailing run, exactly where `&`
   is allowed. A mid-line `*foo:bar` stays body text. Both runs skip
   the `is_trailing_transparent` marker shapes (`@…`, `#…`, `s:`,
   `$`/`$<N>`, schedule, priority, and clip tokens).
2. **Next byte.** The byte after `*` is neither whitespace nor `*`.
   This keeps `**bold**`, `* bullet`, and `5 * 3` literal.
3. **Rest of the token.** The rest of the token contains no `*`, which
   keeps `*urgent*` literal. `validate_note` rejects `*` in the note
   component for both sigils, since Obsidian file names cannot contain
   it (`&no*te:id` and `*no*te:id` are invalid).
4. **Bare `*`.** A bare `*` is a directive (it opens the picker) only
   as the **last token of a line that has other text before it**. A
   line-initial `*` is always literal or a list marker. This differs
   from `&`, whose bare form also works as the first parent token.
5. **Escape.** `\*` escapes it, but only where an unescaped token
   would have been a directive under rules 1–3. Elsewhere `\*` stays
   verbatim Markdown.

Rejections:

- Whole-item `x*…` / `x*` are close park operators. `==*` is not a
  close and teaches `x*`. Link suffixes keep `=x*…`.
- Session operators, `+N`/`-N`, section bullets, project-note
  construction, `!` completions, `--task-ref` pickers, and suffixed
  owners reject `*` exactly as they reject `&`. Error text names the
  sigil the user typed. A claimed `!` keeps precedence
  (`!sase:x *foo` stays the bang teaching error).
- `@route:id=x… *T` and `<text> @route:id=x… *T` are also rejected:
  closing and blocking in one item is incoherent. Capture the close
  first, then `N *T` as a separate blank-line item.

| Draft item | Reading |
| --- | --- |
| `Ask Kelly *job:cleanup` | New inbox task N; `job#^cleanup` depends on N |
| `*job:cleanup Ask Kelly`, `@cash *job:cleanup Ask Kelly` | Same (leading run, past `@cash`) |
| `Ask Kelly @mac_inbox:ask-kelly *job:cleanup` | Same, and N is linked first so it takes T's place in the session |
| `Ask Kelly *job:cleanup *job:send-batch` | N blocks both, in typed order |
| `Call registrar &school:transcript *job:apply-x` | Insert between; cycle-checked |
| `*job:cleanup @bob+ask-kelly`, `@bob:ask-kelly *job:cleanup` | Existing task blocks T (mode `task_dependency`) |
| `^bob:ask-kelly *job:cleanup`, `*job:cleanup ^bob:ask-kelly` | Same relationship with the bare `^` owner alias; no Pomodoro Task Link is written |
| `Order parts *projects/house/kitchen:cabinets`, `X *"Release Plan":publish` | Nested and quoted targets |
| `*job:cleanup` with no owner | Incomplete: needs `dependency_target` (type task text or `+` to choose an existing owner) |
| `*job:cleanup +` | Incomplete: needs `task_parent`; the picker chooses the task that will own the relationship |
| `X *`, `X *jo`, `X *job:` | Incomplete: needs `blocked_task` (picker open) |
| `Fix *urgent* bug`, `Read **Dune**`, `* Buy milk`, `5 * 3`, `R*D`, `Fix *foo:bar now` | Literal |
| `X \*job:cleanup` | Literal `*job:cleanup` |
| `Support *nix` | Partial; submit fails with a teaching error that suggests `\*nix` |
| `N *T &T` | Direct cycle; refused, vault byte-identical |
| `x*1`, `x*` | Close park operators |
| `==*` | Error: `` `==*` is not a close: close the running Pomodoro with `x*` `` |
| `@cash &foo:bar Buy milk` | `&` dependency (leading run skips `@cash`); STP twin of the `*` leading-run rows |

Parse vectors **STP** are the rows of this table plus those extras.
`capture-parse` reports each `*` modifier in per-item and top-level
`blocked_tasks` (`raw`, decoded `note` and `block_id`, `quoted`,
sigil-inclusive `range`) with `blocked_task_sigil`, `blocked_task_note`,
and `blocked_task_block_id` spans. Never put a direction flag on
`dependencies[]`. A complete existing-owner action reports mode
`task_dependency`; new-task and sub-bullet captures keep their mode and
gain the `blocked_tasks` fields. A partial `*` needs `blocked_task`. A
complete `*` with no owner needs `dependency_target`, the same need
`&` uses. Frozen v1 examples (schema version 1, additive fields only):

```json
// bob capture-parse -f json -- 'Ask Kelly *job:cleanup *job:send-batch'
{
  "body": "Ask Kelly",
  "mode": "task",
  "route": null,
  "needs": [],
  "blocked_tasks": [
    {
      "raw": "*job:cleanup",
      "note": "job",
      "block_id": "cleanup",
      "quoted": false,
      "range": { "start": 10, "end": 22 }
    },
    {
      "raw": "*job:send-batch",
      "note": "job",
      "block_id": "send-batch",
      "quoted": false,
      "range": { "start": 23, "end": 38 }
    }
  ],
  "dependency_target": { "kind": "new_task" }
}
```

```json
// bob capture-parse -f json -- '*job:cleanup @bob+ask-kelly'
{
  "body": "",
  "mode": "task_dependency",
  "route": "bob",
  "block_id": "ask-kelly",
  "needs": [],
  "blocked_tasks": [
    {
      "raw": "*job:cleanup",
      "note": "job",
      "block_id": "cleanup",
      "quoted": false,
      "range": { "start": 0, "end": 12 }
    }
  ],
  "dependency_target": {
    "kind": "existing_task",
    "route": "bob",
    "block_id": "ask-kelly"
  }
}
```

`capture-complete` serves context `blocked_task` with a replacement
range covering exactly the active modifier (sigil and quoted note
included), the decoded `query`, the lexical `owner`, and the
open-editable candidate list:

```json
// bob capture-complete -b VAULT -c 13 -f json -- 'Ask Kelly *jo'
{
  "cursor": 13,
  "replacement": { "start": 10, "end": 13 },
  "context": "blocked_task",
  "candidates": [],
  "query": "jo",
  "owner": { "kind": "new_task", "text": "Ask Kelly" }
}
```

Rows reuse the `DependencyCandidate` shape, plus `already_blocked`
(omitted when false) and `disabled_reason`:

- `the owner task itself`
- `would create a dependency cycle`
- `a project cannot depend on its own resident`
- the existing duplicate-block-ID reason

The pool is open, editable tasks only: refuse daily notes, `done/`,
`_templates`, `_generated`, `_conflicts`, dot-directories, and fenced
code. With an empty query, ranking is (1) today's linked tasks (the
running entry's first), (2) Pending `[/]`, (3) Next `[*]`, (4) the
owner's note, (5) everything else. With a non-empty query, ranking is
match score first, then the same order. Replacements use `*`
(`replacement_for_sigil`). Shell completion adds a `blocked tasks`
group.

#### Execution

The whole draft plans before anything is written. For an item with
`blocked_tasks`:

1. **Resolve and check every target.** Dedupe by resolved identity.
   The note resolves vault-wide exactly like `&note`: path or basename,
   nested, quoted, plus staged notes. T must be an open, editable
   task. Refuse targets in daily notes, `done/`, `_templates`,
   `_generated`, `_conflicts`, dot-directories, or fenced code. Also
   refuse a self target; a T with several or malformed Depends-On
   lines; and a `^prj` T when N lives in that project's note or a
   descendant project (a guaranteed deadlock). A repeat is a no-op
   reported as `already_present`.
2. **Give N a block ID.** Use the typed `^id` or `:id`. Otherwise mint
   one with this batch's reservations, even when
   `dependencies.blocks_lines: false`. When the ID was minted, report
   it as `minted_blocker_block_id`. Never reuse
   `minted_dependent_block_id`, which means the opposite (`&`'s BLOCKS
   mirror for N-as-dependent).
3. **Plan N as today.** N's own `&` prerequisites come first, so N may
   be born `[?]`. Insert N and stage its Task Link if `:id`/`#pomodoro`
   asked for one and the existing link guard allows it. This is the
   "link N, then consume T" order: `N @mac_inbox:id *T` keeps the
   session alive by putting N into T's slot before T's link is
   consumed.
4. **Run one writer pass per T, in typed order.** Each pass is the
   existing-dependent writer with dependent = T (vault path plus block
   ID) and prerequisites = [N]. It merges the line and fields, stamps
   T fresh, flips T to `[?]`, blocks and tags `^prj` residents,
   consumes links, and stages N's BLOCKS line. **Commitment transfer:**
   an open N rises to T's pre-capture lane (for a `^prj` T, the
   strongest resident lane), **capped at Next `[*]` when N is new in
   this capture**. The cap applies to the whole promotion walk that
   this pass seeds. An existing owner gets the normal §5 transfer and
   may become `[/]`. A `^prj` N is never raised. Re-locate N by block
   ID after every pass.
5. **Existing owners** go through the dependency-only path. The
   sub-bullet path (`Note text @route+id *T`) and forced
   `--task`/`--route` mirror `&`.
6. **Cycles.** Direct, transitive, cross-item, and `&`+`*` cycles are
   refused; the vault stays byte-identical.
7. **Later link gestures.** A later `@T:id` in the same batch drops N
   from T's line under capture's own-prerequisite drop. That is
   intended, and the final preview reports it accurately. Capture does
   not auto-link N into T's slot; the explicit
   `N @mac_inbox:id *T` idiom covers it.

#### Capture consumption

Every capture path that consumes live links (`plan_existing_task_dependencies`,
`plan_resident_block`, and the link-guard Consume branches) follows
this contract. Reconcile and plugins are unchanged (PDL7).

- **Scope.** Only live (unstruck) Task Links under **today's open**
  Pomodoros (running and planned), every occurrence. Closed entries,
  `🍅` history, other days, prose outside `## Pomodoros`, and
  dependency lines are never touched. A missing day file means there
  is nothing to consume.
- **Identity.** Match by resolved `(vault-relative path, block id)`,
  not by text. Aliases (`|x`), `[[#^local]]`, basename, path, and case
  spellings all match.
- **Notes kept (PDL8).** A consumed **link-only bullet with children**
  is not deleted. Its link token is replaced by the task's clean
  display text: the same `text` capture reports, with checkbox, tags,
  inline fields, and `^id` stripped, and any `[[…#^…]]` inside it
  flattened to plain text. The marker, indentation, and children stay
  unchanged, and the bullet carries no Task Link. A childless
  link-only bullet is deleted. A prose bullet loses only the token
  (PDL6).
- **Only entries this capture emptied (PDL9).** "Pomodoro" means the
  column-0 entry, never the `## Pomodoros` heading. An entry is
  deleted only if it had a child list item before this consumption
  and has none after it, judged after all of the item's edits. That
  includes a running timed entry. Entries that were already childless
  stay.
- **Reported (PDL11).** Each consumed link becomes a `consumed_links[]`
  row. Each deleted entry appears in `pomodoro_blocks` with
  `removed: true`. Human output names the session. The legacy
  `warnings[]` consumption string stays for older clients; human
  output prints the `−` lines instead, so the same fact is never
  printed twice.

#### Human output

These lines are from a real ST3 capture; the final plan-budget footer is
omitted because its limits come from local configuration.

```text
✓ captured  mac_inbox.md
  - [*] #task Ask Kelly for references! [created::2026-10-14] [id:: mac_inbox__ask-kelly-references] ^ask-kelly-references
  🧱 blocks "Do cleanup tasks!"  job.md ^cleanup  [/] → [?] Blocked
  − removed its Task Link from JOB 10:10–10:35 (running) · deleted the empty Pomodoro
```

Variants:

- `− kept its notes under JOB 10:10–10:35 as plain text`
- `· blocked 4 project tasks` for a `^prj` T (dry run: `· would block 4 project tasks`)
- `would …` in dry runs (`🧱 would block "…"`, `🧱 would now block "…"`)
- A typed duplicate `*` target is skipped; a repeat that adds nothing
  prints `🧱 already blocks "Cleanup"  job.md ^cleanup` with no
  status arrow, live and dry-run
- Existing-owner items use the update header and owner status row, then print
  one relationship row per target; for example `✓ updated  body.md`,
  `  [/] → [?]  Exercise  ^excercise`, and
  `  🧱 now blocks "Do cleanup tasks"  job.md ^cleanup  [*] → [?] Blocked`.
  Dry runs use `would update` / `would now block`.

`&` consumption prints the same `−` lines.

Selected fields from the real ST3 `bob capture -f json` result are shown
below; the absolute `target` path and unrelated fields are omitted. The
`created` and `[id::]` properties are part of the emitted task line.

```json
{
  "ok": true,
  "kind": "task",
  "text": "Ask Kelly for references!",
  "relative_target": "mac_inbox.md",
  "block_id": "ask-kelly-references",
  "task_line": "- [*] #task Ask Kelly for references! [created::2026-10-14] [id:: mac_inbox__ask-kelly-references] ^ask-kelly-references",
  "dependency_update": {
    "role": "blocked_task",
    "dependent_note": "job.md",
    "dependent_block_id": "cleanup",
    "dependent_text": "Do cleanup tasks!",
    "new_task": false,
    "added": 1,
    "already_present": 0,
    "open_prerequisites": 1,
    "prerequisites": [
      {
        "note": "mac_inbox.md",
        "block_id": "ask-kelly-references",
        "link": "[[mac_inbox#^ask-kelly-references]]",
        "status_symbol": " ",
        "status_name": "Todo",
        "text": "Ask Kelly for references!",
        "open": true
      }
    ],
    "dependent_status": "?",
    "dependent_status_name": "Blocked",
    "status_changed": true,
    "previous_status": "/",
    "previous_status_name": "In Progress"
  },
  "dependency_updates": [
    {
      "role": "blocked_task",
      "dependent_note": "job.md",
      "dependent_block_id": "cleanup",
      "dependent_text": "Do cleanup tasks!",
      "new_task": false,
      "added": 1,
      "already_present": 0,
      "open_prerequisites": 1,
      "prerequisites": [
        {
          "note": "mac_inbox.md",
          "block_id": "ask-kelly-references",
          "link": "[[mac_inbox#^ask-kelly-references]]",
          "status_symbol": " ",
          "status_name": "Todo",
          "text": "Ask Kelly for references!",
          "open": true
        }
      ],
      "dependent_status": "?",
      "dependent_status_name": "Blocked",
      "status_changed": true,
      "previous_status": "/",
      "previous_status_name": "In Progress"
    }
  ],
  "consumed_links": [
    {
      "day_file": "2026/20261014.md",
      "line": 3,
      "link": "[[job#^cleanup]]",
      "note": "job.md",
      "block_id": "cleanup",
      "text": "Do cleanup tasks!",
      "action": "removed",
      "pomodoro": {
        "line": 2,
        "name": "JOB",
        "time_range": "1010-1035",
        "status": "running",
        "removed": true
      }
    }
  ],
  "minted_blocker_block_id": {
    "path": "mac_inbox.md",
    "line": 1,
    "block_id": "ask-kelly-references"
  },
  "pomodoro_blocks": [
    {
      "relative_target": "2026/20261014.md",
      "line": 2,
      "name": "JOB",
      "time_range": "1010-1035",
      "status": "running",
      "created": false,
      "removed": true,
      "roles": ["removed"],
      "lines": [
        {"text": "- [ ] (1010-1035) — JOB", "depth": 0, "change": "removed"},
        {"text": "  - [[job#^cleanup]]", "depth": 1, "change": "removed"}
      ]
    }
  ],
  "task_blocks": [
    {
      "relative_target": "job.md",
      "route": "job",
      "line": 1,
      "block_id": "cleanup",
      "text": "Do cleanup tasks!",
      "status_symbol": "?",
      "status_name": "Blocked",
      "created": false,
      "roles": ["dependency"]
    },
    {
      "relative_target": "mac_inbox.md",
      "route": "mac_inbox",
      "line": 1,
      "block_id": "ask-kelly-references",
      "text": "Ask Kelly for references!",
      "status_symbol": "*",
      "status_name": "Next",
      "created": true,
      "roles": ["dependency_target"]
    }
  ]
}
```

`dependency_update` equals `dependency_updates[0]`. For `N *T` that is
T's update (dependent T, prerequisites [N]), which older Mac builds
already render as a dependency on T. `&`-only items carry the array
too, with `role: "dependent"`. `blocked_residents` is omitted when 0.
`consumed_links[].action` is `removed`, `token_removed`, or
`notes_kept`. `line` values are 1-based in the item's pre-state day
text. Execution vectors **ST1–ST19** live in
[task-dependencies.md §13.5.2](task-dependencies.md#1352-capture-star-blocks-execution-vectors-st).

<a id="relating-existing-tasks"></a>

### Relating existing tasks with '&' and '*'

`&` adds a prerequisite to the item's owner; `*` makes the named task depend
on that owner. When the item has task text, that text creates the owner. When
it has no task text and names an existing task, that existing task is the
owner. An existing owner is edited in place and is never recreated as a new
bullet.

| Owner spelling | Meaning |
| --- | --- |
| `@route+id` | Canonical explicit owner; may lead or trail the relationship modifiers |
| `@@route+id` | Inherited owner for otherwise ownerless items anywhere in the draft |
| `@route:id` | Existing bare alias; with `&`/`*` it names the owner and never links it |
| `^route:id` | Bare active-task alias; may lead or trail `&`/`*`, and never links it |

The aliases stay narrow: `@route:id` and `^route:id` name an owner only when
used bare with relationship modifiers. Without `&` or `*`, their existing
Pomodoro-link behavior remains unchanged. `&`, `*`, `~`, and `~~` are all
**relationship modifiers**: the first two change status and lanes, while
`~` / `~~` write inert relation lines, except a
[cancel-label](task-relations.md#17-cancel-labels) commit or a
[complete-label](task-relations.md#18-complete-labels) commit
([Linking tasks with '~' and '~~'](#linking-tasks)). The
familiar two-item form still
keeps a blocked task in a session while it takes over the blocker’s slot:

```text
@bob:ask-kelly

@bob+ask-kelly *job:cleanup
```

An owner must be in a location that `@route` can write: the inbox, an area
note, or a non-terminal project note. The prerequisite or blocked task can be
anywhere in the vault. If only one task is routable, flip the relationship so
that task owns it; for example, use
`@cash+budget *projects/house/kitchen:cabinets` instead of trying to own the
relationship from the unroutable project task.

#### Authoring flows

1. **Choose the owner first.** Type `@body+` for the note-scoped picker or `+`
   for the vault-wide picker, choose Exercise, then add `&foo:bar` or
   `*job:cleanup`. In relationship mode, the picker explains which task will
   depend on or block the named task. Self-edges and cycles are disabled with
   Bob’s reason.
2. **Choose the relationship first.** Type `&foo:bar` or `*job:cleanup`, then
   type task text to create the owner. To use an existing owner, type `+` and
   choose it; the replacement is the canonical `@route+id` spelling.
   `capture-parse` reports `dependency_target` while the owner is missing and
   `task_parent` while the `+` picker is open. A draft waiting for an owner
   shows a `dependency_target` hint in Bob Mac Capture instead of a failed
   preview.
3. **Use the current task.** Pick the active task with `^`, then add `&` or
   `*`. The resulting bare `^route:id` names the owner without adding or
   moving a Pomodoro Task Link.

For several prerequisites or blocked tasks, the picker names the first and
counts the rest, for example “Buy shoes + 1 more”. When only one relationship
direction is present, the owner picker says which task will depend on or block
the selected task; with both directions, it names both relationships.

#### Refusals

An unresolved `+` is never captured as task text. On submit it refuses with:

```text
`+` needs a task selection; choose the task to relate before capture
```

Submitting a completed relationship modifier without either task text or an
existing owner also refuses rather than creating a task named after the
modifier. The ownerless `&` and `*` messages are:

```text
task relationships need a dependent: add task text or '@note+task-id' for the task that should use the '&note:block-id' modifier(s)
task relationships need a dependent: add task text or '@note+task-id' for the task that should use the '*note:block-id' modifier(s)
```

The interactive `+` picker is the route for selecting an existing owner.

A relationship-only item with an existing owner edits only the Depends-On and
BLOCKS lines. It cannot carry `$<N>`, `s:<N>`, `%…`, or authored child
bullets because those need task text. Parse emits the ranged error diagnostic
`relationship_needs_text`, and execution returns the same message. The
offending token is named in the message; for example:

```text
`$2` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar $2`)
```

The same template applies to `s:3`, a clipboard token such as `%header`, and
`child bullets`. To write a child bullet, add parent task text to the owner
line, as in `Waiting @body+excercise &foo:bar` followed by `- detail`.

An existing task that is Done or Canceled cannot be the owner of `*`; capture
refuses without changing the vault. The exact error is:

```text
task ^old in foo.md is Done; a finished task cannot block another task (reopen it first)
```

The status word is `Canceled` for a canceled owner. Existing `&` closed-owner
refusal wording is unchanged. Suffixed owners (`#name`, `=…`, or `!`) keep the
existing “must be bare” refusal, since they combine relationships with a
Pomodoro action.

#### Terminal output

An existing-owner relationship uses the update layout: one header, one owner
status row, then one row per relationship. Status arrows appear only when the
status changes. A successful live example is:

```text
✓ updated  body.md
  [/] → [?]  Exercise  ^excercise
  ⛓ now depends on "Buy shoes"  foo.md ^bar  [ ] → [/] In Progress
  🧱 now blocks "Do cleanup tasks"  job.md ^cleanup  [*] → [?] Blocked
  − removed its Task Link from BODY 10:10–10:35 (running) · deleted the empty Pomodoro
```

That live example is one capture of
`@body+excercise &foo:bar *job:cleanup` while Exercise is In Progress and
linked in the running Pomodoro. The final plan-budget footer is omitted
because its limits come from local configuration.

A dry run begins `[dry-run] ok would update  body.md`; relationship rows say
`would now depend on` and `would now block`. An unchanged owner row uses the
toggle wording, for example `  [ ] stays Ready  Exercise  ^excercise`.
Repeated relationships say `already depends on` or `already blocks`, with no
status arrow and no write. The existing `· blocked N project tasks` chip stays
on `🧱` rows.

New-task owners use the same per-relationship row shape, for example:

```text
  ⛓ depends on "Buy shoes"  foo.md ^bar  [ ] Ready
```

For a sub-bullet owner, output prints the bullet and its authored children
first, then the relationship rows. The `under` row shows the parent’s final
status, such as `under [ ] → [?] Exercise  ^excercise`. Consumption continues
to use the structured `−` rows.

#### Completion wire additions

The schema stays version 1 and these fields are additive. In
`capture-complete`, `owner` gains optional `text`, `note_path`,
`status_symbol`, and `status_name` fields. For an existing owner they describe
the resolved task; for `new_task`, `text` is the non-empty draft body.
Unresolved existing owners retain the old owner shape. Drafts without `&` or
`*` keep their current JSON byte-for-byte.

```json
// Selected fields from bob capture-complete -b VAULT -c 17 -f json -- '@body+excercise &'
{
  "context": "task_dependency",
  "cursor": 17,
  "replacement": { "start": 16, "end": 17 },
  "query": "",
  "owner": {
    "kind": "existing_task",
    "route": "body",
    "block_id": "excercise",
    "text": "Exercise",
    "note_path": "body.md",
    "status_symbol": "/",
    "status_name": "In Progress"
  }
}
```

When a complete `&`/`*` modifier is present in a `task_parent` or scoped
`task` picker, its `picker` descriptor gains a `relationship` object. Each
entry carries the typed `raw` marker, exact vault-relative `note_path`,
`block_id`, and resolved task `text`; unresolved entries omit only `text`.
Candidate rows gain `disabled_reason` for self-edges, cycles, and a project
depending on its own resident. A disabled row has no insertable replacement,
and shell completion omits it. These selected fields show the vault-wide `+`
picker in owner mode; its self row is disabled and a cycle row uses the same
guard:

```json
// Selected fields from bob capture-complete -b VAULT -c 10 -f json -- '&foo:bar +'
{
  "context": "task_parent",
  "query": "",
  "replacement": { "start": 9, "end": 10 },
  "picker": {
    "kind": "parent_task",
    "scope": "vault",
    "scope_token": "+",
    "marker_range": { "start": 9, "end": 10 },
    "trigger_removal_range": { "start": 9, "end": 10 },
    "relationship": {
      "dependencies": [
        { "raw": "&foo:bar", "note_path": "foo.md", "block_id": "bar", "text": "Buy shoes" }
      ],
      "blocked_tasks": []
    }
  },
  "candidates": [
    { "replacement": "", "text": "Buy shoes", "disabled_reason": "the prerequisite itself" },
    { "replacement": "", "text": "Cycle candidate", "disabled_reason": "would create a dependency cycle" }
  ]
}
```

#### Conformance vectors

Each **RX** execution vector checks parsing, exact vault bytes, JSON, human
output, dry-run/live agreement, and a second live run as a no-op where
applicable. `foo.md#^bar` is Buy shoes, `body.md#^excercise` is Exercise,
and `job.md#^cleanup` is Do cleanup tasks.

| # | Draft / state | Contract |
| --- | --- | --- |
| RX1 | `&foo:bar @body+excercise`, `@body+excercise &foo:bar` | Exercise depends on Buy shoes; Exercise becomes `[?]`; no new bullet; mode `task_dependency` |
| RX2 | `@body+excercise *job:cleanup` | cleanup depends on Exercise; cleanup becomes `[?]`; Exercise gets the normal commitment transfer (`task-dependencies.md` §5) |
| RX3 | `@body+excercise &foo:bar *job:cleanup` | cleanup → Exercise → Buy shoes |
| RX4 | `^body:excercise &foo:bar`, `&foo:bar ^body:excercise`, `^body:excercise *job:cleanup` | Same relationship as RX1/RX2; parse owner is `existing_task` |
| RX5 | `@body:excercise &foo:bar` | Existing bare `@route:id` alias behavior remains unchanged |
| RX6 | `@@body+excercise` then `&foo:bar`, with or without a blank line | Edge only; no empty bullet; `inherited: true` |
| RX7 | `@foo+old *job:cleanup`, `Note @foo+old *job:cleanup`; owner `[x]` or `[-]` | Refuse with the closed-owner `*` message; vault bytes are unchanged |
| RX8 | `+ &foo:bar`, `+ex &foo:bar`, `&foo:bar +`, `*job:cleanup +ex` | Parse needs `task_parent`, no diagnostics; submit refuses with the exact `+` message above; never create a `+…` task |
| RX9 | `@body+excercise &foo:bar $2`, `s:3`, or `%` | `relationship_needs_text` in parse and the identical execution error |
| RX10 | `@body+excercise &foo:bar` plus child `- note` | Same refusal, naming `child bullets` |
| RX11 | `@body+excercise &foo:bar *foo:bar` | Refuse the cycle with `foo.md^bar -> body.md^excercise -> foo.md^bar` |
| RX12 | `Waiting @body+excercise &foo:bar` plus child `- detail` | Print the sub-bullet and child, then `⛓`; `under` shows `[ ] → [?]` |
| RX13 | `@body+excercise#bugs &foo:bar`, `@body+excercise! *job:cleanup`, `^body:excercise=x &foo:bar` | Existing “must be bare” refusals stay unchanged |
| RX14 | Owner `[/]` linked in the running Pomodoro; `@body+excercise &foo:bar` | Existing consumption applies (`−` row; delete an emptied entry) with the new output layout |
| RX15 | Repeat RX1–RX4 | `already depends on` / `already blocks`; no write |
| RX16 | `Buy milk &foo:bar` | Per-prerequisite row: `⛓ depends on "Buy shoes"  foo.md ^bar  [ ] Ready` |

Completion vectors **RXC** check the additive wire contract:

| # | Draft (cursor at end) | Contract |
| --- | --- | --- |
| RXC1 | `@body+excercise &` | `owner` includes Exercise, `body.md`, and `/` / `In Progress` |
| RXC2 | `^body:excercise &`, `^body:excercise *` | Same owner fields; Exercise is disabled as `the dependent task itself` / `the owner task itself` |
| RXC3 | `Buy milk &` | `owner: {kind: new_task, text: "Buy milk"}` |
| RXC4 | `&foo:bar +` | `task_parent`; `picker.relationship.dependencies[0].text` is Buy shoes; the owner row is disabled as `the prerequisite itself`, and a would-cycle row is disabled |
| RXC5 | `*job:cleanup +` | `picker.relationship.blocked_tasks`; `@job+cleanup` is disabled as `the blocked task itself` |
| RXC6 | `&foo:bar @body+` | Scoped picker returns the same `relationship` object and guards |
| RXC7 | `+ &foo:bar`, cursor after `+` | `task_parent`; replacement covers only the `+` token |
| RXC8 | Any draft without relationship modifiers | Byte-identical to current output: no `relationship` object or new owner fields |

The RXC4 JSON excerpt above freezes the relationship descriptor and guard
strings. The RX8 parse shape for `+ &foo:bar` keeps the incomplete parent
selection and typed modifier, without diagnostics:

```json
// Selected fields from bob capture-parse -f json -- '+ &foo:bar'
{
  "input": "+ &foo:bar",
  "body": "",
  "mode": "incomplete",
  "needs": ["task_parent"],
  "dependencies": [
    {
      "raw": "&foo:bar",
      "note": "foo",
      "block_id": "bar",
      "quoted": false,
      "range": { "start": 2, "end": 10 }
    }
  ],
  "diagnostics": []
}
```

### Linking tasks with '~' and '~~' {#linking-tasks}

`~` links the item's task to another task; `~~` links another task to
the item's task. Both write entries on `🪢 **LINKS:**` lines and let
reconcile maintain the `🪢 **LINKED FROM:**` mirrors, exactly as
[Task relations](task-relations.md) specifies. Owners use exactly the
`&` / `*` spellings: task text creates the owner, `@route+id` names an
existing owner, `@@route+id` is inherited, and bare `@route:id` /
`^route:id` alias without linking. A relation-only existing-owner item
uses mode `task_dependency` and reports `relationship_needs_text`, as
`&` does.

```text
Fix flaky capture retries ~bob_done:capture-star|"follow-up of"
Write the outline ~~sase_blog:prj|"next step"
@bob+fix-retries ~sase:retry-design|see
Fix trip ~work:foo||prompted-by
```

**Meaning.** `~note:block-id` means this item's task relates to that
task. `~~note:block-id` means that task relates to this item's task.
A label after `|` is phrased from the source's side
(`S — ‹label› → T`); write `|"follow-up of"` for multi-word labels.
Labels obey the [relation label rules](task-relations.md#32-labels).
A label listed in `relations.cancel_labels` is a
[cancel label](task-relations.md#17-cancel-labels): committing an
add or relabel into one cancels the relation's source in the same
batch. A label listed in `relations.complete_labels` is a
[complete label](task-relations.md#18-complete-labels): committing an
add or relabel into one marks the relation's source Done in the same
batch.
A label after `||` is a one-off: the stored Markdown is identical to
the `|` form, but the label hides from suggestions once the capture
succeeds. Hiding a visible existing label warns with its link count;
hiding an already-hidden label is a silent no-op. A failed journal
append never fails the capture — it warns with the exact
`bob relations hide` retry command instead.

**Guards.** A `~` / `~~` token is a directive only when it sits in the
same leading or trailing run as `&` / `*`, whitespace-delimited. A
mid-line `~foo:bar` stays body text. The byte after the sigil is not
whitespace or `/` (so `~/Downloads` stays literal), and not a digit
when the token has no `:` (so `~5m`, `~2h`, `~5pm` stay literal).
`~~old~~` is strikethrough, `x~~1` / `=~~2` teach "use one `~` list",
and `=~2` / `x~1` keep their drop-list meaning. A third pipe is a
teaching error (`use || to hide a label, | to name one`). `\~`
escapes the sigil where it would otherwise be a directive; `[[…]]`
and backtick spans are never scanned. An unresolved bare or partial
sigil refuses on submit with `` `~` needs a task; choose one, or write
`\~` for a literal tilde ``.

**Targets.** A `~` target is any task with a block id: open, closed,
any active note, a daily note, or `done/`. A mirror is skipped for
archived and previous-daily targets. A `~~` target's LINKS line is
written, so it must be editable: the refusal set is the same as
`*`'s (daily notes, `done/`, `_templates`, `_generated`,
`_conflicts`, dot-directories, fenced code). Self-relations,
non-task targets, and id-less targets are refused with a teaching
error.

**Refusals.** A relation after `x` is an `operator_relation_error`.
Suffixed owners (`#name`, `=…`, `!`) keep the existing "must be bare"
refusal. A relation-only item cannot carry `$<N>`, `s:<N>`, `%…`, or
authored child bullets (`relationship_needs_text`).

**Shell quoting.** Quote `~` where the shell expands it
(`bob capture 'Fix it ~work:foo'`). In zsh, `~` at a word start can
glob; single-quote the draft.

**Inertness.** Relations never block, consume, schedule, stamp, or
route anything. `Fix it ~work:foo` leaves every status, lane, and
Pomodoro link exactly as it was, apart from a minted `^id`. The
exceptions are a [cancel-label](task-relations.md#17-cancel-labels)
commit, which cancels the source as Task Card Cancel would, and a
[complete-label](task-relations.md#18-complete-labels) commit, which
marks the source Done as `!` would.

**Cancel-label commits.** Adding a cancel label — or relabeling into
one — cancels the relation's source in the same staged batch: the
LINKS edit, `[-]` plus `[cancelled:: today]` (with a Cancel Log
prepend only when a log already exists), today's Pomodoro-link prune,
and dependent recovery share one all-or-nothing batch, so a refusal
writes nothing. `~~` retires the picked task; `~` on the task the
item creates is refused (`relation_cancels_new_task`, with a `~~`
hint), as are recurring sources
(`relation_cancels_recurring_task`) and an item that also changes the
source through `&` / `*`
(`relation_cancel_conflicts_with_item_action`). Already-closed
sources link only. A later item sees an earlier cancel through the
staged batch, so linking, starting, or completing a just-cancelled
task is refused by the existing closed-task checks.

**Complete-label commits.** Adding a complete label — or relabeling
into one — marks the relation's source Done in the same staged batch:
the LINKS edit, `[x]` plus `  [completion:: today]`, today's Pomodoro
Task Link retirement, dependent recovery, and successor offers share
one all-or-nothing batch, so a refusal writes nothing. This is
exactly what `!note:id` runs against the same staged snapshot.
`~~` finishes the picked task; `~` on the task the item creates is
refused (`relation_completes_new_task`, with a `~~` hint), as are
recurring sources (`relation_completes_recurring_task`) and an item
that also changes the source through `=` / `:` / `&` / `*` / `!`
(`relation_complete_conflicts_with_item_action`). A source that one
item or batch would both cancel and complete is refused
(`relation_close_effects_conflict`). Already-closed sources link
only. Offers join the batch's single offer set: numbered, picked with
`-p/--pull-in`, and gated by `--require-pull-in-choice`. A later item
sees an earlier completion through the staged batch, so linking,
starting, or completing it again is refused, or no-ops, exactly as
after a `!`. Planned (phase `complete-capture`).

**Human output.** Successful captures print `🪢` rows in the
dependency style, for example:

```text
✓ captured  mac_inbox.md
  - [ ] #task Fix flaky capture retries [created::2026-10-14]
  🪢 links to "Capture * blocks existing tasks" · follow-up of   done/bob_done.md ^capture-star · archived
```

An existing-owner rerun of the same edge reports `🪢 unchanged`; a
new label reports `🪢 relabeled`. A one-off hide prints
`🪢 hides "see" from suggestions (2 links)`. Dry runs say
`would link to` / `would now link here` / `would relabel`. A staged
cancel adds one bob-authored `⊘` row per source, for example
`⊘ cancelled “Sell stocks” · Ready → Cancelled · removed 1 Pomodoro link · unblocked “Needs stocks”`
(`would cancel` / `would remove` / `would unblock` on dry runs);
an already-closed source prints the muted human line
(`Already Cancelled · link only` / `Stays Done · link only`) instead,
with no ⊘ and no `relation_cancels[]` row.

**JSON.** Every `schema_version` stays 1 and every field is
additive. `capture-parse` adds `relations[]` (top-level and per
item), the `relation_target` / `relation_label` needs, the five
`relation_*` span kinds, and top-level `relation_grammar: 1` whenever
the draft contains a `~` byte. `capture-complete` adds the
`relation_to` / `relation_from` / `relation_label` contexts, with an additive
`relation` object (direction plus the sigil-to-`|` range) and `owner` on
`relation_label` responses.
`capture-task-id` adds `relation_to_replacement` and
`relation_from_replacement`. `capture` adds, per item,
`relation_updates[]`, `linked_from_line_updates[]`,
`minted_relation_block_ids[]`, `mirror_skipped[]`, and
`relation_label_hides[]` (`{label, key, existing_count, was_hidden,
action: hide|already_hidden}`, pre-capture counts, omitted when
empty), plus a `hides_existing_relation_label` warning object
(`{code, label, count, message}`) when a one-off hides a visible
existing label. See [Task relations](task-relations.md#12-kill-switch-outputs-and-warnings).
Cancel-label fields (`relation_cancels[]`, `cancel_effect`,
`relation_cancelled_source`, and `cancels_source` spans) are specified in
[§17 CL7](task-relations.md#cl7-contracts) and
[JSON outputs](reference/json-outputs.md).
Complete-label fields (`relation_completes[]`, `complete_effect`,
`relation_completed_source`, and `completes_source` spans) are specified in
[§18 CP7](task-relations.md#cp7-contracts) and
[JSON outputs](reference/json-outputs.md).

### Completing tasks with '!'

A capture item that is exactly `!note:block-id` marks that existing open
task Done, without closing a Pomodoro. The token names the task with the
same vault-wide note locator `&note:block-id` uses, so `!` and `&` share
one lexer, one resolver, and one replacement formatter: simple root notes
use `!foo:bar`, nested notes use `!projects/foo:bar`, and notes with
whitespace or reserved punctuation use a quoted component such as
`!"Shopping List":milk` (inside quotes only `\"` and `\\` are escapes).
Single-quote the draft on shells that history-expand `!`:
`bob capture '!sase:fix-flaky'`.

A complete token must be the entire capture item: no body text, children,
`%`, `s:`, `$<N>`, `#name`, `=…`, `@@`, or forced destination flags.
Anything extra is a teaching error, never a junk inbox task. Bulk means
one `!` item per blank-line-separated block; every item is planned
against the same staged snapshot, and any failure rolls the whole batch
back.

| Draft item | Reading |
| --- | --- |
| `!` | Query `""`: incomplete, needs `task_complete`, the picker opens |
| `!fix`, `!sase:`, `!"Shopping List`, `!"Shopping List"` | Query: incomplete, needs `task_complete` |
| `!sase:fix-flaky` | Complete: mode `task_complete`; completes `^fix-flaky` in the note `sase` resolves to |
| `!projects/foo:bar`, `!"Shopping List":milk` | Complete: nested and quoted locators, exactly as `&` decodes them |
| `!sase:fix-flaky more`, `!sase:fix-flaky` plus a child line, `!sase:fix-flaky $1` | Claimed and invalid: `invalid_task_complete` |
| `!sase:fix-flaky=x`, `!sase:fix-flaky#bugs` | Claimed and invalid, with the hint "`!` takes no `=` or `#` suffix; to complete the running session's link while closing, use `x!N`" |
| `!sase:a !sase:b` | Claimed and invalid, with the hint "put each completion on its own item, separated by a blank line" |
| `!wow this works`, `! foo`, `Wow!`, `Buy milk !sase:x` | Prose, unchanged: several tokens with an incomplete first token, a space after `!`, or a non-leading `!` |
| `!fix` plus a child line | Prose: an incomplete token on a multi-line item, like `:dee` plus a child |
| `![[sase#^x]]`, `![alt](url)`, `!!`, `!!!` | Prose: embeds, images, and `!` runs are never claimed (the second byte is `[` or `!`) |

`bob capture` refuses a claimed query before any other item parser, with
the usual `capture item K starting on line L:` prefix and exit 2:
`` `!fix` opens the task picker; pick a task to insert its `!<note>:<block-id>` token, or write one yourself (for example `!sase:fix-flaky`) ``.
A claimed invalid item reads
`` `!sase:fix-flaky` completes an existing task and must be the whole capture item; remove `more` ``
(or `remove its child lines`), plus the targeted hints above.
`@@` declarations never apply to a `!` item.

`capture-parse` (schema 1, additive) reports query items with mode
`incomplete`, needs `["task_complete"]`, an empty body, and one
`interactive_placeholder` span over the whole token, sigil included. It
reports complete items with mode `task_complete`, `block_id`, three spans
(`task_complete_sigil` over the `!`, `task_complete_note` over the note
with quotes, `task_complete_block_id` over the ID), and the per-item
`task_complete` object (`raw`, decoded `note`, `block_id`, `quoted`, and
the sigil-inclusive `range`). When the draft has one item and that item is
`task_complete`, the same `task_complete` object is emitted at the top level
of the capture-parse result, mirroring how top-level `dependencies` is
emitted; other drafts omit it. Claimed invalid items report mode
`task_complete` with an `invalid_task_complete` diagnostic, the same mode
handling `invalid_pomodoro_link` uses for a padded `^route:id`.

Execution completes the named task through the shared task-completion
engine, in this order and all against the staged batch snapshot:

1. **Resolve.** The note resolves exactly like `&`: an explicit relative
   path first, else a unique basename (notes staged earlier in the batch
   count too). The block ID resolves through the same lookup, with the
   same missing-with-suggestion, duplicate, and not-a-task errors.
2. **Validate the status.** Ready `[ ]`, Blocked `[?]`, Next `[*]`, and In
   Progress `[/]` complete. An already-Done task is an idempotent no-op
   (`action: "already_done"`, nothing written). Canceled and unknown
   statuses fail with an actionable error.
3. **Refuse recurring tasks.** A line with `[repeat:: …]`, `(repeat:: …)`,
   or `🔁` fails: complete recurring tasks in Obsidian so Tasks writes the
   next occurrence.
4. **Close the tree** exactly as `x!N` writes it: the root gets `x` plus
   `  [completion:: YYYY-MM-DD]` (only when the line carries the
   global-filter tag), and embedded `![[…]]` subtasks close recursively
   (Blocked, recurring, unknown-status, and over-cap descendants are left
   open and reported, never silently dropped). A Blocked descendant and
   everything below it stay open. Unlike `x!N`, the root may
   be Blocked.
5. **Retire Task Links** in today's daily note (when it has a
   `## Pomodoros` section): every completed identity is struck, its bullets
   move to the running entry (else the last completed entry), emptied
   placeholders are removed, and a carried copy is dropped when the
   destination already links the task.
6. **Recover dependents and offer successors.** Blocked `[?]` dependents
   that name a completed task's `[id::]`, have no other open dependency,
   and have no strictly future `scheduled` date recover to their derived
   rank, exactly like Task Status Cycler's Ctrl+Enter recovery. Every
   dependent this close fully unblocked, plus every available resident
   of a project whose `^prj` this close unblocked and each project
   pull-in, is **offered**; only `--pull-in` picks are linked into the
   predecessor's slot and become Next in the same write, per
   [task-dependencies.md §12](task-dependencies.md#12-successor-offers).
   Capture never prompts and never auto-links. Picked successor links
   are placed **before** retirement runs (step 5), so retirement strikes
   or moves the predecessor around them and the vacated slot keeps the
   pick.

Status-group moves, `^prj` frontmatter, `#hide`, and archiving stay with
their existing writers (`bob task reconcile`, `bob projects sync`, and
`bob task archive`); capture never writes them.

Worked example. Before (`sase.md` holds the task, today's daily note holds
the link under the running `CAPTURE` session):

```text
- [*] #task Fix flaky gkeep test ^fix-flaky
```

```text
## Pomodoros
- [ ] (**0920-0950**) — CAPTURE
  - [[sase#^fix-flaky]]
```

`bob capture '!sase:fix-flaky'` writes
`- [x] #task Fix flaky gkeep test  [completion:: 2026-10-05] ^fix-flaky`,
strikes the link in place to `~~[[sase#^fix-flaky]]~~`, and prints:

```text
✓ completed [*] → [x] Fix flaky gkeep test  sase.md ^fix-flaky
  ledger  Task Link struck in CAPTURE · 20261005.md
```

After, the day file reads:

```text
## Pomodoros
- [ ] (**0920-0950**) — CAPTURE
  - ~~[[sase#^fix-flaky]]~~
```

Dry-run JSON equals real-run JSON except for `dry_run`. A dry-run excerpt
for the same capture carries the additive `task_complete` object with its
clean `text` and named ledger entries:

```json
"task_complete": {
  "raw": "!sase:fix-flaky",
  "note": "sase",
  "note_path": "sase.md",
  "block_id": "fix-flaky",
  "action": "completed",
  "completion_date": "2026-10-05",
  "text": "Fix flaky gkeep test",
  "subtasks": [],
  "subtasks_left_open": [],
  "ledger": {
    "day_file": "20261005.md",
    "struck": 1,
    "struck_in": [{"line": 2, "name": "CAPTURE", "status": "running"}],
    "moved": [],
    "deduplicated": 0,
    "dropped": [],
    "removed_placeholders": []
  },
  "unblocked": []
}
```

The batch also carries top-level `pull_in` on every success (`supported:
true`, `offer_count`, `choice`, `picked_count`). Dry-run JSON equals
real-run JSON with the same flags, including `--pull-in`. To pull in
numbered offers, pass `-p 1,2`, `-p all`, or `-p none`:

```text
bob capture -p 1 -- '!sase:fix-apollo'
```

An unknown or stale pick is a teaching error that writes nothing and
lists current offers. Without `-p`, capture recovers and reports
numbered `offer N` rows as not pulled in.

The `ledger` line names entries in this order, joined with `·`, and ends
with the day file: one `Task Link struck in <NAME>` row per entry with an
in-place strike (` (completed)` appended when that entry is completed),
one `Task Link moved <FROM> → <TO> (struck)` row per move, one
`Task Link already in <TO>; dropped the <FROM> copy` row per deduplicated
bullet, and one `removed empty <NAME>` row per removed placeholder. When
an entry has no name, JSON carries `"name": ""` and the human line uses
`line N`. Other variants from the same vocabulary:

```text
  ledger  Task Link struck in PLAN (completed) · 20261005.md
  ledger  Task Link struck in SASE · Task Link moved SASE → CAPTURE (struck) · removed empty SASE · 20261005.md
  ledger  Task Link already in MORNING; dropped the SASE copy · removed empty SASE · 20261005.md
```

Refusals leave the vault byte-identical. Each failure prints
`bob capture: capture item K starting on line L: <message>`; usage errors
exit 2 and I/O-class errors exit 1:

| Refusal | Exit | Message |
| --- | --- | --- |
| Forced destination flags (`--route`, `--section`, `--task`, `--task-section`, `--clip`) on the `!` item | 2 | `` `!sase:solo` completes an existing task and must be the whole capture item; remove its forced destination flags `` |
| Ambiguous note | 2 | `ambiguous note "dup": matches a/dup.md, b/dup.md; use the full relative path` |
| No such note | 2 | `no such note: nosuchnote` |
| Canceled task | 1 | `` `^cx` in sase.md is Canceled; reopen it before completing it `` |
| Unknown status | 1 | `` `^wx` in sase.md has status `[>]`; only Ready, Blocked, Next, and In Progress tasks can be completed `` |
| Recurring task | 1 | `` `^water` in sase.md repeats; complete recurring tasks in Obsidian so Tasks writes the next occurrence `` |
| Missing ID (with a close-match suggestion when one is close) | 1 | `no task with block ID ^missing in sase.md (run 'bob capture-tasks -r sase.md' to list task block IDs)` |
| Duplicate ID | 1 | `block ID ^dup appears 2 times in sase.md; make it unique before capturing` |
| Non-task line | 1 | `^nt1 in sase.md is not a task (line 6: plain line ^nt1)` |

### Routing inbox tasks out

When a capture changes the checkbox of an existing task in `inbox.md` or a
direct area child of `inbox.md`, bob reports it so a client can ask where the
task should go. This comparison runs after the whole batch and successor
recovery have been planned. It includes completions and other status changes
caused indirectly by the batch. Edits to fields or children alone do not
count. A nested status change reports its column-zero `#task` ancestor, since
that is the block a move carries. New tasks, `^prj` tasks, tasks in projects
under an inbox area, and untyped notes under an inbox are not candidates.

Detection vectors (`IR`) run against a sandbox vault:

| ID | Transition | Expected report |
| --- | --- | --- |
| IR1 | Link a Ready task in `mac_inbox.md` | One Ready → Next candidate |
| IR2 | Link an already-Next task and create a new inbox task in the same batch | Neither task is a candidate |
| IR3 | Link a task in the direct `gkeep_inbox.md` child | One candidate classified from its live parent |
| IR4 | Change a nested task, an id-less task, and a task to Done | Report the column-zero parent, stable id-less keys, and the completion |
| IR5 | Change a task in a project under `mac_inbox.md` | No inbox candidate |
| IR6 | Request a route choice with the hidden guard | Exit 1 with `inbox_route_choice_required`; notes stay byte-identical |
| IR7 | Require both pull-in and inbox-route choices | The pull-in refusal wins |
| IR8 | Build destinations with inboxes, nested notes, blocked `^prj`, projects, and excluded folders | Sorted task-move destinations with matching kind, status, parent, and blocked fields |

Apply vectors (`IRA`) answer the candidates with `-i/--inbox-route`:

| ID | Transition | Expected report |
| --- | --- | --- |
| IRA1 | Answer `1=health` for a linked Ready task | The task moves to the area with its Task Link repaired, `fresh` stamped, and `task_blocks`/`open_notes` following it |
| IRA2 | Dry-run and real-run the same answered capture | Identical JSON except for `dry_run` |
| IRA3 | Answer `1=health` for a completed task | The Done task moves home with no stamp |
| IRA4 | Answer an id-less recovered dependent by key | The task moves by exact line with `fresh` stamped and no `unblocked` marker |
| IRA5 | Answer a move whose block ID collides in the destination | The block is renamed and the rename is reported |
| IRA6 | Answer two candidates with two destinations in one bulk draft | Each task moves to its own destination |
| IRA7 | Answer `keep` and `N=` | Both keep the task; kept candidates satisfy the guard |
| IRA8 | Answer one candidate with two destinations | Exit 2; nothing is written |
| IRA9 | Answer an unknown candidate | Exit 2 with `inbox_route_stale` listing the current candidates; nothing is written |
| IRA10 | Answer an inbox destination | Exit 1 with `inbox_route_refused`; nothing is written |
| IRA11 | Answer a project without `## Tasks` | Exit 1 with the engine rule; nothing is written |
| IRA12 | Answer a Blocked `^prj` destination for a Next task | Exit 1 with the engine rule; nothing is written |
| IRA13 | Answer `--pull-in` picks and a route together | Both apply in one batch |
| IRA14 | Route a task a dependency touch tracks | Its `task_blocks` entry follows it and the engine mirror is repaired |
| IRA15 | Change a link-repaired file between planning and commit | The whole batch aborts with the `NoteChanged` failure; nothing is written |

Every successful capture JSON, including dry runs, carries `inbox_route`:

```json
"inbox_route": {
  "supported": true,
  "candidate_count": 1,
  "routed_count": 0,
  "kept_count": 0,
  "candidates": [{
    "key": "mac_inbox.md#^route-task",
    "n": 1,
    "note_path": "mac_inbox.md",
    "inbox": "mac_inbox",
    "block_id": "route-task",
    "line": 8,
    "text": "Prepare the handoff",
    "previous_status_symbol": " ",
    "previous_status_name": "Ready",
    "status_symbol": "*",
    "status_name": "Next",
    "answer": null,
    "outcome": "pending"
  }],
  "destinations": [{
    "relative_path": "areas/health.md",
    "route": "areas/health",
    "label": "health",
    "kind": "area",
    "status": null,
    "parent": null,
    "blocked": false
  }]
}
```

Candidates are numbered by note path and original line. A task with a block ID
uses `path#^id` as its key; an id-less task uses `path@<sha8>` and adds a
`~N` suffix when the display-text digest repeats in that note. Treat keys as
opaque. Candidate status symbols and names describe the task block that would
move. `block_id` is an empty string when the candidate has no block ID.

`destinations` is omitted when there are no candidates. It lists routable area
notes and `wip` or `waiting` projects, sorted by vault-relative path. Inbox
notes and notes under `done/`, `_templates/`, and `_conflicts/` are excluded.
Each row includes the resolved parent label when available and whether the
destination's `^prj` task is Blocked.

`bob capture` never opens a picker. Bob Mac Capture uses this report to ask,
then submits the selected route with the capture. Its hidden
`--require-inbox-route-choice` guard returns exit 1 with
`code: inbox_route_choice_required` and the same `inbox_route` payload when a
candidate has no answer; it writes nothing. The pull-in choice refusal runs
first, so clients resolve successor offers before inbox routes.

Answer candidates with repeatable `-i, --inbox-route ANSWER`:

- `<N|KEY>=<NOTE>` moves the candidate to `NOTE` (a stem or vault-relative
  path, resolved like `bob task move`'s `DEST`, and required to be in the
  destination set).
- `<N|KEY>=` keeps the candidate in its inbox.
- `keep` keeps every candidate not otherwise answered.

Two different destinations for one candidate are a usage error (exit 2).
An answer naming an `N` or `KEY` that is not a current candidate fails with
exit 2 and `code: inbox_route_stale`, listing the current candidates exactly
as `-p` does for stale picks; it writes nothing. An answer whose destination
does not resolve or is refused fails with exit 1 and
`code: inbox_route_refused`, adding `refused` (`key`, `n`, `destination`,
`message`) beside `inbox_route`; it writes nothing. Conflicting answers,
stale answers, and refusals never write. Without the guard flag, unanswered
candidates stay where they are and the real run still succeeds with
`outcome: pending`.

Answered moves run as a new stage inside capture planning: after every item,
the successor net post-pass, and candidate detection, and before the task
and Pomodoro block trackers finish. The capture acts first, then moves each
answered candidate through the `bob task move` engine (`--inbox-route` is
the only caller that moves closed selections or selects by exact line) in
one atomic batch: a full vault snapshot overlaid with the staged texts,
per-group move plans, the freshness stamp on each moved root line, and the
same link repair, collision renames, and preimage guard as any other batch
write. A moved open task is stamped `[fresh:: today]` like a Ctrl+Shift+M
move; a moved Done task is never stamped.

The move updates the batch outputs to their final, post-route text.
`answer` becomes `{"action": "move", "destination": "<relative_path>"}` or
`{"action": "keep"}`; `outcome` becomes `moved` or `kept`. A moved
candidate gains `moved` (`destination`, `line`, `block_id`,
`previous_block_id`, `fresh`, which is the stamped date or `null`). Once a
move is applied, `inbox_route` also carries `repaired_files` (`path`,
`link_count`, `dependency_token_count`), `renamed_block_ids` (`path`,
`old_block_id`, `new_block_id`), and `needs_reconcile`. A routed task's
`task_blocks` entry follows it to the destination path and its final lines
and block ID, and an `open_notes` entry that would land on a routed task
opens the destination at the moved line instead.

The human output prints one `inbox` row per pending candidate and a route
hint, one `kept  <text> · still in <inbox>` row per kept candidate, and one
`routed  <text>  <inbox> → <route>` row per moved candidate, with
`· ^old renamed ^new` and `· N links repaired` suffixes when they apply. In
dry-run output the rows say `would route`; ordinary previews never write.
The `run 'bob task reconcile' to reconcile Blocked state` hint prints when
`needs_reconcile` is true.

### Plan budget and strict mode

When a batch changes today's Pomodoros section, the result carries a
top-level `plan_budget` with before/after meters (dry-run and real runs
use the same planner, so they agree). It is never per item, and it is
omitted when the ledger is unchanged:

```json
"plan_budget": {"status": "over",
  "themes": {"count": 4, "cap": 3, "over": true, "before": 3},
  "links": {"count": 8, "cap": 10, "over": false, "before": 7},
  "added_themes": ["BOB"],
  "added_theme_refs": [{"key": "bob", "name": "BOB", "hue": "magenta",
    "hue_source": "hash"}],
  "warnings": [{"code": "plan_theme_cap_exceeded",
    "message": "today's plan now has 4/3 themes (adds BOB); queue it with ^ or defer with $<N>"}]}
```

Warnings fire only for `plan_theme_cap_exceeded` and
`plan_link_cap_exceeded`, and only while the batch grows that meter past
its cap (`after > cap` and `after > before`). A capture that leaves an
over-cap plan alone stays quiet. Human output prints one meter line
after the result (`plan 4/3 themes · 8/10 links  (+1 theme: BOB)`) and
one `bob capture: warning: …` stderr line per fired warning; the
warnings never join the item `warnings` array. An invalid plan config
skips the budget and pushes one plain string into `warnings` instead.

`added_theme_refs` is ordered with `added_themes`. Human Task Link
destinations add a colored square before each known, non-exempt component
when stdout is a color terminal; `NO_COLOR` keeps the existing destination
text unchanged.

With `plan.strict: true`, the whole batch is refused, atomically, when
a non-start item creates a new named Pomodoro entry past the theme cap
(`<text> @r:id#NAME`, `@r^id+#NAME` project notes, `@r+id#NAME`
toggles, and any other non-start creation path). Session starts and overrides
(`=`, `=<X>#NAME`, `#NAME=`, and link starts) are never refused. A named start
that creates a theme still reports `plan_budget` and the cap warning. The
refusal is an
I/O-class error (exit 1) whose JSON carries a machine-readable `code`:

```json
{"ok": false, "error": "refusing capture: today's plan would grow to 4/3 themes (adds BOB); …", "code": "plan_theme_cap_exceeded"}
```

`code` appears on this refusal and on `pull_in_choice_required`
(§12.14 when `--require-pull-in-choice` is set, offers exist, and no
`--pull-in` was given); every other capture failure omits it.

Every `pomodoro_link_destination` carries a `role` naming how the entry
was chosen: `current` (the running timed entry), `next_up` (implicitly
chosen and not running), `named` (an existing entry matched by `#NAME`),
or `created` (a new entry). New `<text> @route:id[#NAME]`
Pomodoro-task captures report `pomodoro_link_destination`,
`pomodoro_name`, and `creates_pomodoro` too, and human output names the
landing spot: `→ into running GOALS (0945-1015)`, `→ under GOALS (next
up)`, `→ under GOALS (named)`, or `→ new Pomodoro BOB`. In the
`pomodoro_name` completion context, `creates_pomodoro` rows preview the
result with `plan_themes_after` and `plan_themes_cap`, omitted when the
daily note or the plan config is unavailable. In the `pomodoro_start_name`
context, the new and again rows carry `plan_themes_after` and
`plan_themes_cap` too.

### Starting the next Pomodoro

Capture a whole item `=`/`=<X>` to start today's next future Pomodoro now:

```bash
bob capture '='
bob capture '=3'
bob capture '=-2'
printf 'x\n\n=\n' | bob capture
```

Pomodoro sessions form a lifecycle, taught in this order:

| Item | Operator | Effect | Obsidian |
| --- | --- | --- | --- |
| `=` / `=<X>` | start / restart | Start the next future Pomodoro, or restart today's running Pomodoro with fresh `se<X>` timing (`=` is 25 minutes); an override shows a warning before the write | `se<X>` + Tab inside `()` or on the running line |
| `=<X>#<pomodoro>` | start named / swap | Start that named Pomodoro (open match in place, completed match again, else created); while another session runs, swap in a different name, keeping its clock with an empty `<X>` or using fresh timing when `<X>` is given | `se<X>` + Tab inside `() — NAME` or on the running line after picking the name |
| `+[N]` / `-[N]` | resize | Move the running session's end N × 5 min later / earlier | `N\p` / `N\P` |
| `++[N]` / `--[N]` | shift | Move the running session's start and end N × 5 min later / earlier | `N\o` / `N\O` |
| `x` | close | Close the running session | Ctrl+Enter |

Mnemonic: "`=` starts the next session — or restarts the running one; `=name` starts that one — or swaps it in; `x` stops the running one."

Recognition: let `t` be the item's first physical line trimmed of leading
and trailing whitespace (the token after a chain split; see
[Chaining session operators on one line](#chaining-session-operators-on-one-line)).
A start token is `=` followed by the longest run
matching the `se<X>` suffix grammar `[0-9]*(-[0-9]*)?`. A token is bare when
its suffix is empty or contains no digit (`=`, `=-`) and counted when its
suffix contains at least one digit (`=3`, `=-2`, `=3-`, `=2-1`, `=0`).
The whole-item close is recognized first, as a bare lowercase `x` token
(plain `x`, selection-shaped `x…`, or the `x#…` near miss), so a start token
never consumes a close. An exact start (`t` is exactly one start token and the item has
exactly one physical line) goes through the existing `se<X>` timing, so `=`
is 25 minutes, `=3` is 15 minutes, `=-` is 25 minutes with a 5-minute
offset, `=2-1` is 10 minutes with a 5-minute offset, `0` and leading zeros
are valid, and an oversized value fails before any write. A counted token
followed by anything else (`=3 more`, `=-2 @work`, `=3s:1`, `=2-1-`, `=3x`)
or an exact token with child lines fails as an invalid start, never a task.
A bare token followed by more text (`= foo`, `=- foo`, `=-)`), bare
`x` lookalikes (`xx`, `xa`,
`X`, `X2`), and mid-body tokens (`Plan =3`, `Plan =bugs`, `Plan x`, `a=3`) stay ordinary
prose. `=xx` and `=xa` are named starts now, not prose. The retired `==` spelling stays prose when followed by more text,
which protects Obsidian highlight syntax; a claimed `==` start token gets
the retirement error and teaches the matching `=` spelling.

Guards, checked in order on the staged daily note: a missing day file, a
missing `## Pomodoros` section, more than one open timed entry, and no future
Pomodoro (a bare `=` never creates an entry;
`=name` creates and starts a named session (see "Starting a named
Pomodoro"), and `^route:block-id=` starts a task's session). "Next future
Pomodoro" means the first entry
in document order that is open, a placeholder, and untimed — the same session
the `x` diagnostic names as "next up". If one open timed entry already
runs, a whole-item `=` restarts it as described in
[Overriding the running Pomodoro](#overriding-the-running-pomodoro); link
starts keep their separate refusal.

Selection, time, rewrite, and transaction mirror Obsidian's `se<X>` + Tab:
the `()` becomes the canonical `(**HHMM-HHMM** [t:: Nm])` range, preserving
checkbox, name, other bytes, children, CRLF, and a missing final newline,
then the entry moves to the current slot. No entry is created, no link is
added, and no task note is written. Later items see earlier staged edits and
dry-run computes without committing; any failure rolls the whole batch back.
Composable idioms: `x`, blank line, `=` switches sessions (or `x =` on one
line; the `x` closes before `=` plans, so no override warning appears);
`=`, blank line, `+2` starts then extends; `=`, blank line,
`^bob:ready` starts then links a
task into the now-running session. A `@@` declaration never applies to start
items, and forced `--route`/`--section`/`--task`/`--task-ref`/
`--task-section`/`--clip` fail on them; `%`, `s:<N>`, and `$<N>` after a
counted token are shape errors by construction.

The started session reports its queued Task Links (direct-child Task Links
of the started entry, resolved read-only). The lineup is numbered 1..N in
ledger order on the staged pre-image, so every whole-item start shows which
number a queued link holds: JSON kind `pomodoro_start` keeps
schema version 1 with `text` = the raw token, `task_line` = the started
ledger line, `placement: "started"`, and the existing `pomodoro_start`
object with `created_pomodoro: false` plus an additive `tasks` array where
each row carries its `index` (its lineup number). The
started session also appears in the batch-level `pomodoro_blocks` array;
see [Pomodoro blocks](#pomodoro-blocks).
Human output prints the started file, the session range and line, the
canonical ledger line, one numbered row per queued task, and `nothing queued`
when empty.

Quote `=` items in zsh, which expands a leading `=word` to a command path:
`bob capture '='`, `bob capture '=3'`, `bob capture '=-2'`.
Single-quote close tokens that contain `!`, `*`, or `~` (`bob capture 'x!1'`,
`bob capture 'x*'`).

#### Dropping queued Task Links as a session starts

Write a trailing `~<K>` drop list on a whole-item start to start lean:
`=~2` starts the next session without queued Task Link 2. One-card
mnemonic: **`~` drops** — `x~2` drops task 2 from the session you stop,
`=~2` drops task 2 from the session you start, using the numbers the start
lineup shows.

`<K>` is comma-separated task numbers, each at least 1, with no whitespace,
in any order; JSON reports the list sorted ascending. The drop part always
comes last: after `<X>`, and after `#name` when present. A named start's
name part ends at the first ASCII whitespace or `~`. Any start token with a
drop part claims its item, near misses included; a bare `=` followed by
separate text (`= ~2`, `= foo`) stays prose. Link and task starts
(`^route:id=<X>`, `@route:id=<X>`) do not take a drop part.

The lineup is the started entry's direct-child Task Links in ledger order,
numbered 1..N on the staged pre-image (after earlier batch items and chain
tokens have run). Every whole-item start reports these numbers, including a
plain `=`. A named start that creates its session has an empty lineup, so
any drop list fails before insertion.

Each dropped Task Link bullet is removed together with its nested child
lines (which belong to that link in a session about to start — unlike a
close, which keeps notes as history). No task note is written and no lane
changes: a dropped task keeps its status. Dry-run computes
without writing, and any failure rolls the whole batch back.

JSON kind `pomodoro_start` keeps schema version 1: `text` is the raw token,
`pomodoro_start.drop` is the typed list (omitted when empty),
`pomodoro_start.tasks[]` are the rows still queued (each with `index`),
and `pomodoro_start.dropped[]` are the removed rows (with `index`,
the pre-image `ledger_line`, and `nested_lines`, omitted when 0).
Human output numbers the queued rows, prints dropped rows inline (`dropped
2 [[T]] · stays Next · +1 nested line`), follows with a dim `Dropped <K>`
summary, and prints `nothing queued` when nothing is left.

Worked example (`BOB_NOW=2026-09-30 09:42:00`, TAB indentation;
`^capture-stop`, `^web-capture`, and `^axe-restart` are `[*]`):

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
  - 🍅 [[bob#^capture-stop]]
- [ ] () — CAPTURE
  - [[bob#^capture-stop]]
  - [[bob#^web-capture]]
    - remember the URL parser
  - [[sase#^axe-restart]]
- [ ] () — SASE
```

`bob capture '='` numbers the lineup 1 `^capture-stop`, 2 `^web-capture`, 3
`^axe-restart`. `bob capture '=~2'` writes:

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
  - 🍅 [[bob#^capture-stop]]
- [ ] (**0945-1010** [t:: 25m]) — CAPTURE
  - [[bob#^capture-stop]]
  - [[sase#^axe-restart]]
- [ ] () — SASE
```

(`bob.md` and `sase.md` are untouched.)

### Starting a named Pomodoro

Capture a whole item `=name` to start one named Pomodoro now (`=#name` is the same token):

```bash
bob capture '=bugs'
bob capture '=3#bugs'
bob capture 'x =bugs'
```

A named start token is `=` plus an `se<X>` suffix (possibly empty) plus a name: either a bare name right after `=` (`=bugs`), or `#` plus a name part (`=#bugs`). A bare name starts with an ASCII letter, is not close-shaped, and does not start with `x#`/`X#`; it ends exactly where a `#name` part ends (whitespace, `~`, or `@`). The `#` spelling is still always valid: write `#` only when a number comes first (`=3#bugs`) or the name starts with a digit or reads as a close (`=#2fa`, `=#x`, `=#x2`). `=bugs` and `=#bugs` lex to one token and produce the same `CaptureKind`, vault writes, and `capture-parse` semantics; only `input`/`body` echoes and span offsets differ. The `<X>` goes before `#` or last after `@` (`=bugs@3` means `=3#bugs`). The name part is a Pomodoro
selector with the usual charset (`A-Z`, `a-z`, `0-9`, `& ' ( ) + , . / -`);
write spaces in names as `-`. To spell the name exactly, quote it after `#`:
`=#"DEEP WORK"` (or `=#“DEEP WORK”` with curly quotes) names `DEEP WORK`
verbatim, matched by key equality instead of the whole-slug/else-prefix
rule, so `SASE V18` and `SASE-V18` stay distinct and `=#"BOB"` creates `BOB`
even with `BOB BUILD` open. Bob always emits straight quotes; `\"` and `\\`
are the only escapes inside the quotes. The token runs through the closing
quote, and a literal without its closing quote (`=#"DEEP`) is an incomplete
editing state, never a mistake. The bare `=name` spelling never takes quotes:
`="BOB"` is ordinary prose. Every `#name` position takes the literal —
named starts, link suffixes (`@route:id#"…"`, `@route+id#"…"`,
`^route:id#"…"`), project-note bullets, toggles, and close positions — so one
rule holds everywhere. A named start claims its item and is never
prose: an exact token (the item's only text, on exactly one physical line)
starts the session, and anything else is a precise error. Unchanged: `=`
without a name, every close form, `= #foo` and `= bugs` (a bare `=` with prose stays prose),
mid-body tokens (`Plan =bugs` stays prose; put any word before it to capture a task that must begin with `=word`, and use a child bullet for a Work Log entry that must end in `=word`), doubled `==` (which keeps protecting Obsidian highlights), and the link forms
(`^route:id#name=<X>` keep their `#name=<X>` order alongside the new `=name` suffix). `x#…` is a claimed
close near miss with a teaching error; it used to be prose.

Resolution runs against the staged daily note, so chains and batches see
earlier edits. Open entries match first (whole slug, else prefix), then
completed ones the same way:

| Match | Effect |
| --- | --- |
| Open untimed placeholder | Starts in place: the `()` becomes the canonical range and the entry, with its child block, moves to the current slot; `created_pomodoro: false` |
| Completed entry ("again") | A new session is created with that entry's canonical name and started; history is never modified; `created_pomodoro: true` |
| No match | A new session is created with the selector's canonical name and started; `created_pomodoro: true` |

Creation inserts the placeholder after the last completed entry's block,
otherwise before the first entry, otherwise at the section start — the same
placement the named link start uses — and the subsequent move is then a
no-op. An invalid name fails before anything is written, and when the name
is only a near miss of an open entry the capture still succeeds but pushes
a `did you mean …?` warning naming the open entry. A created entry reports
`tasks: []`; a started open entry reports its direct-child Task Links
through the existing queued lineup, exactly like an unnamed start, with
the same `tasks[].index` numbering.

Guards, checked in order: a missing day file (the message still names the
canonicalized selector), a missing `## Pomodoros` section, name resolution,
then more than one open timed entry. With exactly one timed entry, a different
named target swaps in behind the same override warning as `=`. A counted
start naming the running Pomodoro restarts it with the requested timing;
an empty suffix naming that same session is refused because it would not
change which session runs:

```text
`=#capture` names CAPTURE, which is already running (0920-0945, line 5): restart it with `=` or `=<X>`, close it with `x`, or name another Pomodoro to swap in
```

When a running session must stay untouched, Bob Mac Capture passes the hidden
`--refuse-override` guard until the warning has appeared in its preview. A
matched open entry that is not an untimed placeholder or the running target
fails with the existing selected-Pomodoro wording instead of starting.

With `BOB_NOW=2026-07-10 09:02:00` and a ledger holding completed `PLAN`,
open `BUGS` (with `[[sase#^deep-fix]]`), open `DEEP WORK` (with
`[[bob#^outline]]` and `[[bob#^draft]]`), and an open unnamed entry:

| Item | Result |
| --- | --- |
| `=deep-work` | `DEEP WORK` becomes `(**0905-0930** [t:: 25m])` and moves ahead of `BUGS`; queued: `^outline`, `^draft` |
| `=deep` | Identical (prefix match) |
| `=3#bugs` | `BUGS` becomes `(**0905-0920** [t:: 15m])` and stays in place |
| `=bugs@3` | Identical to `=3#bugs` |
| `=plan` | A new `PLAN` session is created and started (the old `PLAN` is completed history) |
| `=#"DEEP WORK"` | `DEEP WORK` starts exactly as spelled, by key equality rather than slug or prefix |
| `=#"BOB"` with `BOB BUILD` open | A new `BOB` session is created and started instead of selecting `BOB BUILD` |

The bytes contract matches the unnamed start: checkbox, name, children,
CRLF, and a missing final newline are preserved. Dry-run computes the same
result without writing, and any failure rolls the whole batch back. JSON
kind `pomodoro_start` keeps schema version 1 with `text` = the raw token,
`task_line`, `placement: "started"`, the existing `pomodoro_start` object
plus `pomodoro_name`, `created_pomodoro`, and the additive `tasks` array.
The started session also appears in the batch-level `pomodoro_blocks`
array; see [Pomodoro blocks](#pomodoro-blocks).
Human output prints `started` (dry-run: `would start`), the session name,
range, and line with ` (created)` when the entry was created, the canonical
ledger line, one numbered row per queued task, and `nothing queued` when
empty. A `@@` declaration never applies to named starts, and forced
`--route` /
`--section` / `--task` / `--task-ref` / `--task-section` / `--clip` fail on
them with the existing start error. Quote named starts in zsh like every
other `=` item: zsh's `EQUALS` expansion rewrites a word that starts with `=`, so `bob capture =focus` fails unquoted. Always single-quote (`bob capture '=bugs'`, `bob capture 'x =bugs'`, `bob capture '=#"DEEP WORK"'`).

### Overriding the running Pomodoro

A whole-item `=` / `=<X>` restarts the single running session with fresh
`se<X>` timing. A whole-item `=name` starts or swaps in the named
session. The warning appears in the preview before the write, so the person
can see which running session will be replaced. Link starts such as
`@route:id=<X>` and `^route:id=<X>` keep their existing refusal while a
session is running; only a whole-item start overrides.

```bash
bob capture '='
bob capture '=3'
bob capture '=-2'
bob capture '=bugs'
bob capture '=3#bugs'
```

For a restart, `=` means 25 minutes, `=3` is 15 minutes, and `=-2` is 25
minutes with a 10-minute offset. The running session stays the same; Bob
replaces its parenthesized time ledger and moves it to the current slot. A
trailing `~<K>` drops Task Links from the session that remains running,
numbered from the start lineup. Restarting is allowed even when the running
session is overdue or has notes; `plan.strict` never refuses a start or
override.

The dry-run warning is printed ahead of the result. With the worked example
below, Bob prints these exact lines:

```text
⚠ overrides the running CAPTURE 0920-0945 · capture `x =` to close it first
CAPTURE 0920-0945 → 0935-1000 (25m) at line 5
```

A swap names the target in its close-first draft:

```text
⚠ overrides the running CAPTURE 0920-0945 · capture `x =bugs` to close it first
```

When the session is overdue, the warning says so:

```text
⚠ overrides CAPTURE 0920-0945, still open past its end · capture `x =` to close it first
```

For an unnamed running session with Task Links, the warning, the swap
demoted line, and the `--refuse-override` error name its derived homes
chip after the range:

```text
⚠ overrides the running session 0920-0945 ○ sase · capture `x =` to close it first
session 0920-0945 ○ sase → first future at line 7 · keeps 2 Task Links
```

An unlinked unnamed session prints exactly as before.

A named start resolves an open whole-slug match first, then a prefix match;
a completed match starts again with that name, and no match creates it. When
a different session is running, `=bugs` swaps `BUGS` in and hands it the
running session's ledger byte-for-byte (`ledger: "kept"`); `=3#bugs` swaps
it in with fresh 15-minute timing. `=5#bugs` explicitly swaps in with fresh
25-minute timing. The old session returns intact to the first future slot,
including its links and notes. Drops apply to the session that ends up
running. A swap follows the same warning and guard as a restart.

Naming the running session with an empty suffix is refused because it would
not change which session runs. Add timing to restart it, or choose another
name to swap in. For example, Bob reports:

```text
`=#capture` names CAPTURE, which is already running (0920-0945, line 5): restart it with `=` or `=<X>`, close it with `x`, or name another Pomodoro to swap in
```

With exactly one open timed entry, the `pomodoro_start.override` object is
present on the `pomodoro_start` result. Its `action` is `restart` or `swap`;
`ledger` is `fresh` for a restart or a timed swap, and `kept` for an empty-
`<X>` swap. `previous` describes the running session before the write;
`pomodoro_name` is omitted for an unnamed session. `overdue` is omitted when
false and `true` once the current time reaches the session end. `close_first`
is the draft that would close the running session first. An unnamed session
also carries the derived R9 `homes` object (forecast) on `previous` and, for
a swap, on `demoted`; named sessions do no resolution and omit it:

```json
"override": {
  "action": "restart",
  "ledger": "fresh",
  "previous": {"pomodoro_name": "CAPTURE", "pomodoro_line": 5,
               "start": "0920", "end": "0945", "duration_minutes": 25,
               "time_range": "0920-0945"},
  "close_first": "x ="
}
```

An overdue session adds `"overdue": true` to `previous`. A swap adds
`demoted`, describing the old session in its first-future position, and
uses the matching close draft:

```json
"override": {
  "action": "swap",
  "ledger": "kept",
  "previous": {"pomodoro_name": "CAPTURE", "pomodoro_line": 5,
               "start": "0920", "end": "0945", "duration_minutes": 25,
               "time_range": "0920-0945"},
  "demoted": {"pomodoro_name": "CAPTURE", "pomodoro_line": 7,
              "entry_line": "- [ ] () — CAPTURE", "task_links": 2,
              "has_notes": false},
  "close_first": "x =bugs"
}
```

A plain start with nothing running has no `override` object. The result's
existing `pomodoro_start` fields describe the session now running; the
batch-level `pomodoro_blocks` reports it with role `started` and a swapped-
out session with role `reset`.

Bob Mac Capture passes hidden `--refuse-override` when the on-screen preview
for the current draft has not shown an override warning. If the state changes
before submit, Bob refuses with `code: pomodoro_override_refused`; the client
shows a fresh preview and requires another Return. This guards the warning
seen before a commit without adding a confirmation step. For an unnamed
running session with Task Links the refusal names the derived chip after the
range (`would restart the running session (0920-0945 ○ sase) and
--refuse-override is set`); named and unlinked sessions read as before.

Worked example (`BOB_NOW=2026-10-09 09:32:00`, TAB indentation):

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
  - 🍅 [[bob#^capture-stop]]
- [ ] (**0920-0945** [t:: 25m]) — CAPTURE
  - [[bob#^capture-stop]]
  - [[bob#^web-capture]]
- [ ] () — SASE
- [ ] () — BUGS
  - [[sase#^fix-flaky]]
```

| Item | Result |
| --- | --- |
| `=` | CAPTURE becomes `(**0935-1000** [t:: 25m])` in place |
| `=3` | CAPTURE becomes `(**0935-0950** [t:: 15m])` in place |
| `=-2` | CAPTURE becomes `(**0925-0950** [t:: 25m])` in place |
| `=~2` | CAPTURE restarts at `0935-1000` without `[[bob#^web-capture]]` |
| `=bugs` | BUGS takes over `(**0920-0945** [t:: 25m])` and moves under PLAN; CAPTURE becomes `- [ ] () — CAPTURE` right after BUGS, both links intact; SASE follows |
| `=3#bugs` | Same layout, BUGS is `(**0935-0950** [t:: 15m])` |
| `=plan` | A new PLAN session ("again") takes over `0920-0945`; CAPTURE first future |
| `=capture` | Refused: CAPTURE is already running; restart it with `=` or `=<X>`, close it with `x`, or name another Pomodoro to swap in |
| `=3#capture` | Identical to `=3` (a restart) |
| `=bugs +2` | Swap, then BUGS extends to `0920-0955` |

`=bugs` writes:

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
  - 🍅 [[bob#^capture-stop]]
- [ ] (**0920-0945** [t:: 25m]) — BUGS
  - [[sase#^fix-flaky]]
- [ ] () — CAPTURE
  - [[bob#^capture-stop]]
  - [[bob#^web-capture]]
- [ ] () — SASE
```

A swap resolves the name against the pre-image, applies `~<K>` to the
incoming session's lineup, then demotes the running session. Demoting clears
the running ledger to `()` and moves the whole block before the earliest
other open untimed placeholder, so the old session is first future. The
headline suffix and every child byte stay intact, including Task Links,
struck links, nested details, and stand-alone notes. There is no note gate,
history row, task-note write, or Work Log. For an empty `<X>`, the incoming
session receives the previous ledger bytes verbatim, including range-local
annotations; for a non-empty `<X>`, it receives fresh timing. A note-free
restart leaves the day file byte-identical to `x0 =<X>`; a kept-ledger swap
matches `x0 =#name` except that the target receives the old running ledger.

### Adjusting the current Pomodoro

Capture a whole item `+[N]` or `-[N]` to adjust today's current timed
Pomodoro by N five-minute units:

```bash
bob capture +5
bob capture -- -2
bob capture +
printf '+5\n\nCall bank @Cash+\n' | bob capture
```

`+5` extends by 25 minutes and `-2` shortens by 10 minutes. The count is
optional and defaults to 1, so a bare `+` or `-` is one unit rather than an
incomplete state. The item must contain only the signed count (or share its
line only with other session operators; see
[Chaining session operators on one line](#chaining-session-operators-on-one-line)):
trimmed text
matching `^[+-][0-9]*$` with a positive magnitude. Leading/trailing
whitespace is fine. `+0`/`-0` fail, and oversized magnitudes fail checked
parsing/arithmetic before any write. If a valid signed-count token starts an
item but has extra text, a marker, or a child line, the item fails as an
invalid adjustment instead of creating a task; `Plan +5` stays ordinary
prose.

The adjustment selects exactly one open timed, column-zero entry in today's
`## Pomodoros` section. Missing/unreadable day file or section, no open
timed entry, multiple open timed entries, or an unparseable target range
fail without writing. The start stays fixed; `new_duration =
max(0, old_duration + signed_units * 5)` with checked arithmetic and
`new_end = (start + new_duration) modulo 24 hours`. Only that ledger line is
rewritten to Bob's canonical `(**HHMM-HHMM** [t:: Nm])` form, preserving its
checkbox, name, non-duration metadata, child bullets, newline style, and
surrounding bytes. Subtraction clamps at zero and reports both requested
units and actual applied minutes when clamping changes the delta.

A `@@route` declaration still routes ordinary items in the same draft but
never turns `+5` into a task or changes its destination. Forced
destination/task/section and clipboard options (`--route`, `--section`,
`--task`, `--task-ref`, `--task-section`, `--clip`, `%`, `s:<N>`, `$<N>`)
are rejected on adjustment items with a specific error. Later items observe
earlier staged edits to the same daily file through `CaptureBatchPlanner`,
so a `+5` after a session-start item can adjust the session created earlier
in the draft; dry-run computes the same result without committing, and a
later invalid item rolls back all earlier staged changes. The same holds on
one line: `+2 x` extends then closes atomically.

JSON keeps every existing key and schema version 1 stable and reports a
distinct `pomodoro_adjust` kind with an additive `pomodoro_adjust` object
(direction/requested units, actual delta minutes, before/after
start/end/duration, resolved line/name, rendered new range). The adjusted
session also appears in the batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks). Human output
names what changed and where; dry-run says what would change.

### Shifting the current Pomodoro

Capture a whole item `++[N]` or `--[N]` to shift today's running timed
Pomodoro N five-minute units later or earlier, keeping its duration:

```bash
bob capture ++3
bob capture -- --2
bob capture --1
printf -- '--2\n\n+\n' | bob capture
```

One sign moves the end; two signs move the whole session. `++3` translates
both endpoints 15 minutes later and `--2` translates both 10 minutes
earlier, exactly like Obsidian's `3\o` / `2\O` with the running Pomodoro
selected. The count is optional and defaults to 1, so `++` and `--` move one
unit. The item must contain only the operator (or share its line only with
other session operators; see
[Chaining session operators on one line](#chaining-session-operators-on-one-line)):
trimmed text matching
`^(\+\+|--)[0-9]*$` with a positive magnitude. Leading/trailing whitespace
is fine. `++0`/`--0` fail ("move nothing"), oversized magnitudes fail
checked parsing/arithmetic before any write, and a counted token with extra
text, a marker, or a child line (`++3 more`, `++3@work`, `--2 s:1`) fails as
an invalid shift instead of creating a task. A bare sign run followed by
more text (`-- aside`, `++ plan`), a run longer than two or mixed (`+++`,
`---`, `+-`), and mid-body tokens (`Plan ++3`, `C++`) stay ordinary prose.

Target selection mirrors the adjustment: exactly one open timed,
column-zero entry in today's `## Pomodoros` section. Both endpoints
translate modulo 24 hours (`new_start = (start + delta) mod 1440`,
`new_end = (end + delta) mod 1440`), so shifts wrap across midnight and
never clamp; a `++288` wraps a full day. There is no overlap check against
neighbouring entries. Only that ledger line is rewritten to Bob's canonical
`(**HHMM-HHMM** [t:: Nm])` form, preserving its checkbox, name,
non-duration metadata, child bullets, newline style, and surrounding bytes.
A `@@route` declaration never applies to shift items, and forced
destination/task/section and clipboard options (`--route`, `--section`,
`--task`, `--task-ref`, `--task-section`, `--clip`, `%`, `s:<N>`, `$<N>`)
are rejected on shift items with a specific error. Later items observe
earlier staged edits, dry-run computes without committing, and any failure
rolls the whole batch back, so `--2` then `+` in one draft composes (or
`--2 +` on one line).

The shell sees a leading `-` as a flag: spell an earlier shift as
`bob capture -- --2`, or `bob capture --1` for one unit. A bare `--` stays
the end-of-options marker and carries no text; pipe the draft
(`printf -- '--\n' | bob capture`) when that reads better.

JSON keeps every existing key and schema version 1 stable and reports a
distinct `pomodoro_shift` kind with an additive `pomodoro_shift` object
(direction, requested units, signed delta minutes, before/after
start/end/duration, resolved line/name, rendered new range). The shifted
session also appears in the batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks). Human output
names what shifted and where (`shifted 2026/20260928.md`, `FOCUS 0900-0925
to 0915-0940 (25m), 15m later at line 5`); dry-run says what would shift.

### Closing the running Pomodoro

Capture a whole item `x` (lowercase `x` only) to close today's running
timed Pomodoro:

```bash
bob capture 'x'
bob capture 'x2'
bob capture 'x1!2'
bob capture 'x0'
bob capture '^bob:capture-stop=x'
bob capture '^bob:capture-stop=x!1'
printf -- '-2\n\nx\n' | bob capture
printf 'x\n\n=\n' | bob capture
```

Single-quote tokens that contain `!`, `*`, or `~`: shells glob `*` and
history-expand `!`. A bare `x` needs no `=`-word quoting.

#### Retired close spellings

Whole-item `=x…`, `=*…`, and `=!…` are claimed teaching errors. They write
nothing. Each message names the typed token and the exact bare-`x` rewrite.
Link and task suffixes keep `=x…`; only the short `=*` / `=!` aliases on
those suffixes retire, rewriting to `=x*` / `=x!`.

| You typed | Error |
| --- | --- |
| `=x` | `` `=x` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x` `` |
| `=X` | `` `=X` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x` `` |
| `=x2!1` | `` `=x2!1` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x2!1` `` |
| `=x wired the lexer` | `` `=x` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x wired the lexer` `` |
| `=x#bugs` | `` `=x#bugs` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x =bugs` `` |
| `=*` | `` `=*` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x*` `` |
| `=*1` | `` `=*1` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x*1` `` |
| `=*!2` | `` `=*!2` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x*!2` `` |
| `=!` | `` `=!` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x!` `` |
| `=!2` | `` `=!2` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x!2` `` |
| `=x =` | The `=x` token fails on its own range: `` `=x` is retired: close the running Pomodoro with a bare `x`, without the `=`; write `x` ``; the trailing `=` still parses as a start |
| `^bob:stop=*` | `` `=*` is retired: a link or task close spells the `x`; write `=x*` `` |
| `@r:id=!2` | `` `=!2` is retired: a link or task close spells the `x`; write `=x!2` `` |
| `==x` | `` `==x` is not a close: close the running Pomodoro with `x` `` |
| `==X1` | `` `==X1` is not a close: close the running Pomodoro with `x1` `` |
| `==*` | `` `==*` is not a close: close the running Pomodoro with `x*` `` |
| `==!2` | `` `==!2` is not a close: close the running Pomodoro with `x!2` `` |

Mid-body `Plan =x` stays ordinary prose. `=xx` and `=xa` are named starts now, not prose. Capitalizing the
first letter (`X post about the launch`, `X11 forwarding`, `X *job:cleanup`)
is the escape for a task that must start with the word `x`.

This ports Obsidian's Ctrl+Enter Pomodoro completion (task-status-cycler's
`completeActivePomodoroTask`), not the Ctrl+Shift+Enter task-level pause:
the ledger ends up byte-identical to Obsidian's completion except for the
auto-decrement below, the deliberate atomicity divergences, and the
intentional capture-only `x0` reset exception (a note-free `x0` resets to
first future instead of completing; Obsidian completion remains separate). A session
closed before its planned end is shortened to the stop time
(`BOB_NOW=2026-09-28 09:37:00` closing `0920-0950` writes `0920-0940 [t::
20m]`): `remaining = end − now` normalized into (−720, 720] minutes, and
when `remaining ≥ 5`, `units = floor(remaining / 5)`,
`new_duration = max(0, duration − 5·units)`,
`new_end = (start + new_duration) mod 1440`, rewritten in the same canonical
`(**HHMM-HHMM** [t:: Nm])` form `+N`/`-N` writes. Otherwise the range bytes
are untouched; Bob never extends a session, and an overrun is reported
(`ran 7m over`). Closed at `09:49` or later, `0920-0950` stays unchanged.

The close's child lines are Work Log bullets, and one entry may sit on the
close line itself (or it shares its line only with other session operators;
see
[Chaining session operators on one line](#chaining-session-operators-on-one-line)
and [Logging work while closing](#logging-work-while-closing));
`xx`, `xa`, `x.`, `x-ray`, `xylophone`, capitalized `X…`, and mid-body
`Plan x` stay ordinary prose, as does mid-body `Plan =x` and `Plan =bugs`.
Capitalizing the first letter is the escape for a task that must start with
the word `x`. `x more` logs the inline entry `more` to task 1, `x86 build`
claims task 86, and `x *job:cleanup` is refused as a Pomodoro-operator
relationship. (`==x` is a claimed near miss teaching the `x` close, never
prose.) A bare
`=` is now a whole-item start, not an incomplete state. When no session is
running but a future Pomodoro exists, the close diagnostic names it with a
`(start it with `=`)` hint. `@route:block-id=x…` and
`^route:block-id=x…` first put that existing task into the running session
(Ready/Blocked become Next; the Task Link moves or is appended into the
session's sub-bullet range) then close it; `<text>
@route:block-id=x…` creates the new Next task in the running session then
closes it. When the §13.5 link guard finds the existing-task target Blocked,
§13.5.1 drops its own prerequisites first when every safety gate passes, then
the ordinary link-and-close path runs. Otherwise the status stays as-is (a
Blocked task never rises on a history link) while the link still lands under
the closing entry as history; the open prerequisites rise and the result
reports `link_guard` with `drop_refused` when an own dependency reason kept
the prerequisites. Whole-item `x#name` fails (`` `x` always closes the running Pomodoro; remove `#name`, or write `x =name` to close it and then start that Pomodoro; `=name` swaps it in without closing the running session ``). Link `#name=x` fails (`` `=x` always closes the running Pomodoro; remove `#name` (drop `=x` to link under a named Pomodoro instead) ``). So do `s:<N>`/`$<N>`, project-note `+` forms, and forced flags on a whole-item close. A `@@` declaration never applies to
close items.

Sub-bullet classification (after stripping 🍅 markers): fenced lines stay
untouched notes; `![[…#^id]]` embeds are closed recursively — except Depends-On
lines, which are never tree-closed, so closing a dependent never closes its
prerequisites — and retired to `~~[[…]]~~`; a body that is exactly one plain block link plus `#` is
deferred (removed from the session, carried without the `#`, target not
started); other plain block links are worked-on (kept, given exactly one
`🍅 `, carried); a worked-on line that is exactly one bare link is
startable (`[ ]`/`[*]` → `[/]`, stamping `[fresh:: TODAY]` in the same write;
see `docs/freshness.md`); everything else stays as an uncarried note. Completing
a task (`!` selection → `[x]`) never stamps.
Carried lines (worked-on then deferred, in source order, at original
indentation) move into a new `- [ ] ()` (or `- [ ] () — NAME`) placeholder
inserted after the session's sub-bullet range, created iff something is
carried or no later entry follows; with nothing carried the placeholder gets
a `\t- ` stub. Direct-child qualifying links with descendants write dated
Work Log entries under a `🛠️ **WORK LOG**` marker (newest on top) or append
one after the task's child block. Notes stay in the ledger (copied, not
moved). Close-time recovery plus successor **offers** run on every close
that completes tasks, per
[task-dependencies.md §12](task-dependencies.md#12-successor-offers):
picked successors go into the same-name continuation, created when
needed, and count as carried. Without `--pull-in`, capture recovers and
reports numbered offers (`not_selected`). The continuation rule is the
same-name entry search in §12.3: the created continuation first, else
the first open same-name entry after the closed one, else a new
placeholder at the continuation position. Completed-reference
retirement **outside** the session still stays with reconcile.

Diagnostics (all write nothing): a missing day file, a missing Pomodoros
section, no open timed entry (`` no running Pomodoro to close ``, plus
``; next up is NAME at line N`` when an open placeholder exists), several
open timed entries, and an unparseable-range warning that still closes
without decrementing. The start guards now append ``(close it with `x`)``.

JSON keeps schema version 1 and reports kind `"pomodoro_close"` (link and
task forms keep their kind) with an additive `pomodoro_close` object
(timing, per-target transitions, carried links, notes, next session). A
whole-item `x` close also reports the closed session — and the next
session it names — in the batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks).
Field notes:

- `pomodoro_close.raw` is the typed token exactly as typed: the bare token
  (`x`, `x1*2!3~4`) for whole-item closes, and the `=`-prefixed suffix
  (`=x!1`) for link and task closes.
- `pomodoro_close.in_progress` is the typed `<N>` list sorted ascending,
  `null` when no `<N>` was typed (`x0` reports `[]`, and `x*2` keeps `null`),
  `pomodoro_close.park` is the typed `*<P>` list (possibly empty, omitted when
  empty), `pomodoro_close.complete` is the typed `!<M>` list (possibly empty), and
  `pomodoro_close.drop` is the typed `~<K>` list, omitted when empty.
- `pomodoro_close.log` is the typed Work Log entries in typed order
  (bullets or the one inline entry;
  `[{ "index": 2, "text": "wired the lexer" }]`), omitted when empty.
  In `bob capture` JSON `index` is always the task the entry logged to,
  including for unnumbered bullets, which execution resolves positionally.
  Each entry carries `details`, the nested detail lines typed under it
  (`[{ "index": 2, "text": "wired the lexer", "details": ["chose a hand-rolled lexer"] }]`),
  omitted when the entry has none.
- `pomodoro_close.tasks[].typed_work_log` holds the dated entries
  (`*YYYY-MM-DD* — text`, a subset of `work_log`) that this capture's typed
  entries produced, in typed order, omitted when empty.
  `pomodoro_close.tasks[].typed_work_log_details` is aligned 1:1 with it:
  element _i_ lists the detail lines written under typed entry _i_ (undated
  sub-bullets of the dated entry, never counted in `+N Work Log`), with an
  empty list for entries without details; omitted when no typed entry on
  the row has details.
- `pomodoro_close.task_links` is the numbered lineup, always present and
  possibly empty, in number order. Each entry carries `index` (1-based),
  `ledger_line` (the close's pre-image, after any link step), `block_link`
  (never including the `!`), `block_id`, `marker` (`plain`, `deferred`, or
  `embedded`: the line's marker before the selection), `outcome`
  (`in_progress`, `parked`, `deferred`, `complete`, or `dropped`), and `source`
  (`ledger` when
  the outcome comes from the line's own marker, `listed` when its number
  was typed, `unlisted` when `<N>` was typed without it).
- `tasks[].index` is the number of the numbered line that produced the row,
  or `null` for unnumbered rows (struck, mentioned, subtask, and
  Work-Log-only); when one task is numbered on several lines, its single
  row carries the lowest number. `tasks[].role` keeps its current meaning:
  it is the role after the selection, so a completed row is `embedded`, a
  deferred row is `deferred`, and a dropped row is `dropped`. A parked row
  stays `worked` (or its existing unresolved role): parking never changes task
  resolution or status semantics, only carry.
  A client joins `tasks[].index` to `task_links[index-1]` for
  `outcome`/`source`.
- `tasks[].carried` is true only when that target's line was actually
  carried into the new placeholder, and agrees with the top-level
  `carried` list, the next-session count, rendered blocks, and plan budget.
  Parked rows report `carried: false` with no other independently carried
  reference to that target.
- Link forms (`pomodoro_link`, `pomodoro_task`) keep the placement their
  non-close counterpart reports; only whole-item `x` uses
  `placement: "closed"`.
- Inside `pomodoro_close`, explicitly `null` (not omitted) appears for
  `tasks[].warning`, for `tasks[].relative_target`, `text`,
  `previous_status_symbol`, `previous_status_name`, `status_symbol`, and
  `status_name` on unresolved rows, for `next_pomodoro.time_range`, and for
  `next_pomodoro` itself when null — matching the top-level
  `route: null` / `scheduled: null` convention.
- Embedded and subtask row `text` omits the `#task` tag, like every other
  row.
- Whole-item `x` reports `created` as the clock's date string, as every
  other kind does.
- `pomodoro_close` carries the same `unblocked[]`, `still_blocked[]`,
  and `unblocked_check` fields as `task_complete` (see the
  task-complete result contract above): the same row shapes, the same
  `offer` object, the same `not_linked` reasons (`not_selected`,
  `no_destination`, `project_member`; never `breaker` or
  `not_planned_today`), the same `trigger` / `hide_removed` fields, and
  the same gate semantics over the close's completed set. Dry-run JSON
  equals real-run JSON with the same flags. The batch also carries
  top-level `pull_in` on every success.
- When the successor logic created the continuation, `next_pomodoro`
  reports it (`{line, name, time_range: None, created: true}`); picked
  successors count as carried.
- Added block lines that equal a surviving picked successor bullet carry
  `"reason": "unblocked"` in the `pomodoro_blocks` line objects; rows
  dropped by net batch reporting lose the reason.

Human output prints `closed NAME old → new (Nm, −Xm) · file line N` (or a
single range with no decrement, plus `(ran Nm over)` on overruns), always
naming the day file; one line per task as `transition text route ^id`
plus `+N Work Log` counts; dropped rows read `dropped <K> [[T]]`
(with a `stays <status>` lane caption: a dropped task keeps its status) and a
`Dropped <K>` summary follows the rows; the same numbered `offer N` /
`linked` / `unblocked` / `still blocked` successor rows the `!` close
prints, after the task rows;
and the next session, which reads
`carries 1 link` in the singular. When picked successors rode along, `next:`
names them (`next: BOB (created) at line N · carries K links ·
1 unblocked`). When at least one row is numbered, every
task row is prefixed with a right-aligned index column (width = digits of
the highest number) plus one space; unnumbered rows get blanks of the same
width and Work Log preview lines indent to stay under the text. The index
is bold, colored by outcome (in progress uses the `/` color, complete uses
the `x` color, deferred is dim), and unlisted indices are dim too. With
zero numbered rows the output is byte-identical to before. The "switch tasks" batch
idiom is `x`, blank line, `^route:id=` — close the running session,
then start the next one through the existing start rules. The session
switch idiom is `x`, blank line, `=` (or `x =` on one line) — close the running session, then
start the next future Pomodoro.

Worked example (`BOB_NOW=2026-09-28 09:37:00`, day file
`2026/20260928.md`, TAB indentation):

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
	- 🍅 [[bob#^capture-stop]]
- [ ] (**0920-0950** [t:: 30m]) — CAPTURE
	- [[bob#^capture-stop]]
		- Designed the `x` grammar
			- chose `x` for done
		- Wrote the plan
	- [[bob#^web-capture]]#
	- ~~[[sase#^axe-restart]]~~
		- Restarted axe
	- quick note
- [ ] () — SASE
	- [[sase#^recovery-panel]]
```

`bob capture 'x'` rewrites the day file to:

```markdown
## Pomodoros

- [x] (**0830-0855** [t:: 25m]) — PLAN
	- 🍅 [[bob#^capture-stop]]
- [x] (**0920-0940** [t:: 20m]) — CAPTURE
	- 🍅 [[bob#^capture-stop]]
		- Designed the `x` grammar
			- chose `x` for done
		- Wrote the plan
	- ~~[[sase#^axe-restart]]~~
		- Restarted axe
	- quick note
- [ ] () — CAPTURE
	- [[bob#^capture-stop]]
	- [[bob#^web-capture]]
- [ ] () — SASE
	- [[sase#^recovery-panel]]
```

`bob.md` starts `^capture-stop` (`[*]` → `[/]`) with a `🛠️ **WORK LOG**`
of the two dated design notes; `^web-capture` was deferred so it stays
`[*]`. `sase.md` prepends `*2026-09-28* — Restarted axe` under the
existing Work Log marker.

Other rows on the same fixture: `x` at 09:49 keeps `0920-0950 [t::
30m]`; `^bob:ready=x` appends after `quick note` (`linked`, carried
second); `^bob:ready=x3` does the same link step, then closes with the two
existing links deferred and the new `[[bob#^ready]]` in progress as number
3; `^sase:recovery-panel=x` moves the subtree from SASE
(`moved`); `^bob:capture-stop=x` is `already_current` and identical to
plain `x`; `^bob:capture-stop=x!1` is `already_current` and completes it;
`Draft docs @bob:draft-docs=x` creates the new task;
`Draft docs @bob:draft-docs=x0` creates it deferred;
`-2`, blank line, `x` decrements once (or `-2 x` on one line); `-2`, blank line, `x5` rolls back
the `-2`; `x`, blank line,
`^sase:recovery-panel=` switches tasks; `^bob:ready#capture=x` errors
with "remove `#capture`"; a second `x` reports "no running Pomodoro to
close… next up is CAPTURE at line 13"; `x more` logs the entry `more` to
task 1; `x3` is out of range on this fixture.

#### Choosing each Task Link's outcome

Close the running session with `x[<N>][*<P>][!<M>][~<K>][@<N>]` (lowercase
`x` only, with `*`, `!`, `~`, and `@` in any order) to decide, by number, which of its
Task Links stay in progress, which are deferred, which are completed and
struck, and which are dropped — in one capture instead of editing the
`[[…]]#` / `![[…]]` markers by hand before Ctrl+Enter (single-quote the
argument: shells glob `*` and expand `!`):

```bash
bob capture 'x2'
bob capture 'x*2,3'
bob capture 'x!1'
bob capture 'x1*2,3!4,5'
bob capture 'x0'
bob capture 'x~4,5'
bob capture 'x1*2!3~4'
bob capture 'x0*2'
bob capture 'x*'
bob capture 'x!2'
```

Parking records normal In Progress work without carrying those links into
the next Pomodoro (`x*2` works on and parks 2; `x1*2,3!4,5` continues 1,
parks 2 and 3, completes 4 and 5). The five-link acceptance close
`'x1*2,3!4,5'` keeps normal `🍅` history for 1, 2, and 3, gives 1, 2, and 3
exactly the normal In Progress and Work Log effects, retires 4 and 5 as usual,
and carries only link 1 into the new placeholder. Parked rows read
`Parked · not carried` and preserve their actual status transition.

`<N>`, `<P>`, `<M>`, and `<K>` are comma-separated task numbers with no
whitespace. `<N>` omitted leaves unlisted links at their ledger outcome
unless `*<P>` is present, which activates selection mode like `<N>`;
`<N>` present, even as a lone `0`, turns every explicitly unassigned link
that would have been in progress into deferred. A present `*` or `!` group
without numbers is a wildcard for every numbered link left after explicit
in-progress, park, complete, and drop assignments. Thus `x*` parks all links,
`x1*` keeps task 1 in progress and parks the rest, and `x*!2` parks the
rest while completing task 2. Explicit forms such as `x*1` and `x!1` keep
the old single-task scope. `x0*` parks all links because `0` assigns none
to in-progress; an empty lineup still closes successfully. Two empty
wildcards compete regardless of marker order; add task numbers to at least
one group. An absent group stays absent and `~` never
defaults. `*0`, `!0`, and `~0` are invalid. Empty comma elements never
select all (`x*,2` fails, `x*1,` dangles). Order inside a list does not
matter, and `*<P>`, `!<M>`, and `~<K>` may each appear at most once, in
any order after an optional initial `<N>`. `*` and `!` introduce outcome
groups; only omitting that group's numbers makes it a wildcard. A selection
is the marker edits the user would make by hand, applied to the numbered
lines, plus explicit carry metadata for parked links (normal work without
carrying), followed by the unchanged close — so plain `x` works byte for
byte as it always has.

**Numbering.** Every line of the running session's sub-bullet range whose
list-item body, after stripping 🍅 markers, is exactly one block link is
numbered in ledger order starting at 1, at any depth:

| Body shape | Marker | Outcome if nothing is listed |
| ---------- | ------ | ---------------------------- |
| `[[T]]` | plain | in progress |
| `[[T]]#` | deferred | deferred |
| `![[T]]` | embedded | complete |
| `![[T]]#` | embedded (the `#` is inert, as today) | complete |

Struck links (`~~[[T]]~~`, already done), lines that mix a link with other
text (the "mentioned" role, never started), notes, and fenced lines are
never numbered. `T` is any `path#^id` block link with an optional `|alias`,
copied verbatim. On link forms (`@route:block-id=x…`,
`^route:block-id=x…`, `<text> @route:block-id=x…`) the numbering is taken
after the link step: a newly linked or created task lands last and gets the
highest number, while an already-current task keeps its place.

**Outcomes.** For numbered line _i_ with marker _m_, the first matching row
wins:

| Condition | Outcome | Source |
| --------- | ------- | ------ |
| _i_ ∈ `<P>` | parked | listed |
| _i_ ∈ `<M>` | complete | listed |
| _i_ ∈ `<K>` | dropped | listed |
| _i_ ∈ `<N>` | in progress | listed |
| `<N>` typed or `*<P>` present and _m_ = embedded | complete (a hand transclusion is kept) | unlisted |
| `<N>` typed or `*<P>` present | deferred | unlisted |
| otherwise | the ledger outcome of _m_ | ledger |

A number in two lists fails lexically, as does an out-of-range number.
Only numbered lines whose outcome differs from their marker's ledger outcome
are rewritten, in place, keeping indentation and list markers: in progress
becomes `[[T]]`, deferred becomes `[[T]]#`, complete becomes `![[T]]`, and
dropped becomes `~[[T]]` (a transient marker the ledger planner removes:
the line leaves the closed session, is not carried, and its task is never
started). 🍅 markers are dropped from rewritten lines (the close adds back
exactly one on worked lines); line count never changes and matching lines
stay byte-identical.

| Outcome | 🍅 in the closed entry | Carried to the next placeholder | Task effect |
| ------- | ---------------------- | ------------------------------- | ----------- |
| in progress (`N`, or the ledger default) | yes | yes | started `[/]` |
| parked (`*P`) | yes (same normal work history and child notes) | no | started `[/]` (same normal In Progress and Work Log effects) |
| deferred | no (removed) | yes | none |
| complete (`!M`) | embedded | no | closed |
| **dropped (`~K`)** | **no (removed)** | **no** | **none** — the task keeps its lane |

Parking is a close outcome, not a new task status: `[/]` keeps its In
Progress name, and the existing sticky-lane PENDING behavior provides the
waiting place. Carry suppression is explicit planner metadata, not a durable
Markdown marker. Unlisted plain links with `<N>` or `*<P>` present defer;
unlisted embedded links complete. `x!2~3` without `<N>` or `*<P>` keeps
ledger outcomes for unlisted links.

An omitted `<N>` keeps the ledger default for unlisted links. A typed
`<N>` defers the unlisted ones, exactly as today. Dropped links count as
"not carried" when the close decides whether to create a placeholder.

**Worked selections.** On the fixture above, the numbered Task Links are 1 =
`[[bob#^capture-stop]]` (line 6, plain, with nested notes) and 2 =
`[[bob#^web-capture]]#` (line 10, deferred).

`bob capture 'x2'` defers 1 and keeps 2 in progress:

```markdown
- [x] (**0920-0940** [t:: 20m]) — CAPTURE
		- Designed the `x` grammar
			- chose `x` for done
		- Wrote the plan
	- 🍅 [[bob#^web-capture]]
	- ~~[[sase#^axe-restart]]~~
		- Restarted axe
	- quick note
- [ ] () — CAPTURE
	- [[bob#^web-capture]]
	- [[bob#^capture-stop]]
```

`^capture-stop` stays `[*]` but still gets its two-entry Work Log;
`^web-capture` becomes `[/]`.

`bob capture 'x1!2'` keeps 1 in progress and completes 2:

```markdown
- [x] (**0920-0940** [t:: 20m]) — CAPTURE
	- 🍅 [[bob#^capture-stop]]
		- Designed the `x` grammar
			- chose `x` for done
		- Wrote the plan
	- ~~[[bob#^web-capture]]~~
	- ~~[[sase#^axe-restart]]~~
		- Restarted axe
	- quick note
- [ ] () — CAPTURE
	- [[bob#^capture-stop]]
```

`^capture-stop` becomes `[/]` with its Work Log; `^web-capture` closes as
(two spaces before `[completion::`, as today's embedded close writes it):

```text
- [x] #task Add capture support for web URLs! [created::2026-09-21]  [completion:: 2026-09-28] ^web-capture
```

Human output (`NO_COLOR`):

```text
✓ closed CAPTURE 0920-0950 → 0920-0940 (20m, −10m) · 2026/20260928.md line 5
  1 [*] → [/] Add support for `x` syntax! bob.md ^capture-stop +2 Work Log
      *2026-09-28* — Designed the `x` grammar
      *2026-09-28* — Wrote the plan
  2 [*] → [x] Add capture support for web URLs! bob.md ^web-capture
    [x] Restart axe sase.md ^axe-restart +1 Work Log
      *2026-09-28* — Restarted axe
  next: CAPTURE (created) at line 14 · carries 1 link
```

`bob capture 'x0'` defers everything:

```markdown
- [x] (**0920-0940** [t:: 20m]) — CAPTURE
		- Designed the `x` grammar
			- chose `x` for done
		- Wrote the plan
	- ~~[[sase#^axe-restart]]~~
		- Restarted axe
	- quick note
- [ ] () — CAPTURE
	- [[bob#^capture-stop]]
	- [[bob#^web-capture]]
```

Both tasks stay `[*]`, and `^capture-stop` still gets its Work Log. `x1`
equals plain `x` on this fixture (2 was already deferred); `x1,2`
un-defers 2 so both links get `🍅`, both tasks become `[/]`, and the
placeholder carries `[[bob#^capture-stop]]` then `[[bob#^web-capture]]`.

A note-free `x0` instead resets: it clears the current headline's
parenthesized session ledger to `()` (including `[t:: ...]`, legacy
duration notation, and range-local annotations), keeps the headline suffix
and every child byte, and moves the whole block before the earliest other
future placeholder so it becomes first future. No history row is created,
no task is started/completed/released, and no Work Log is copied. Nested
details under a dedicated task link stay with that task; any direct prose
note (including a note after a blank, an empty bullet with authored
descendants, or meaningful indented/fenced content outside a dedicated
subtree) keeps the existing close. Explicit modifiers (`x0*2`, `x0!2`,
`x0~2`, wildcards) always close. After a reset, `x0 =` restarts the reset
session and a second `x0` reports next-up.

**Diagnostics (all write nothing).** Out of range:
`` `x4` names task 4, but CAPTURE has 2 numbered Task Links (1–2) `` (`(1)`
for one link; `` `x1` names task 1, but CAPTURE has no numbered Task Links;
close it with `x` `` for none; an unnamed session reads "the running
Pomodoro"); several bad numbers are listed together. Conflicting duplicates:
``tasks 1 and 3 both link `[[bob#^a]]` but get different outcomes; give them
the same one`` (same-outcome duplicates are fine). Malformed lists fail
lexically: ``task 1 is listed twice in `x1,1` ``,
``task 1 cannot both stay in progress and park in `x1*1` ``,
``task 1 cannot both stay in progress and complete in `x1!1` ``,
``task 1 cannot both stay in progress and drop in `x1~1` ``,
``task 1 cannot both park and complete in `x*1!1` ``,
``task 1 cannot both park and drop in `x*1~1` ``,
``task 2 cannot both complete and drop in `x!2~2` ``,
`` `0` means no task stays in progress; use it alone, as `x0`, `x0*2`, `x0!2`, or `x0~2` ``
(for `x0,2`, `x0,`, and `x00`), `task numbers start at 1` (for `*0`, `!0`,
and `~0`), `` expected a task
number before `,` ``, `` expected a task number after `,` `` (for `x1,!2`), `` use one `*` list: `x1*2,3` ``, `` use one `!` list: `x1!2,3` ``,
`` use one `~` list: `x1~2,3` ``,
``` `x1a` is not a task list: write `x`, then comma-separated task numbers,
then optionally `*` and the numbers to park, `!` and the numbers to complete,
and `~` and the numbers to drop (for example `x1*2!3~4`) ```,
`task number 99999999999 is too large`, and
``write the task numbers right after `x`, with no spaces (for example
`x1*2!3~4`)``. Competing empty wildcard groups report
`` `x*!` has competing park-all and complete-all groups; add task numbers to at least one group ``.
A present-but-empty `*`/`!` group selects the remaining links (so `x!~2`
completes every link except dropped task 2, while `x*!2` parks every link
except completed task 2). Explicit forms such as `x*1` and `x!1` keep the
single-task scope. A token ending in a dangling separator (`x1,`, `x*1,`, `x~`, `x*~`, `x*2,`, `x!2,`) is an editing state: `bob capture` rejects it
(`` `x1,` is incomplete: type a task number after `,` ``) while
`capture-parse` reports mode `incomplete` needing `pomodoro_close_task`. A trailing `*`/`!` is valid, never dangling.
**Warnings** (shown on the row and top-level, never blocking): a listed
in-progress line whose task did not end In Progress
(``task 2 `[[bob#^x]]` is Blocked, so it was not started``) and a listed
complete line whose task did not end Done. In a batch, a selection failure
rolls the whole batch back.

#### Logging work while closing

##### One entry on the close line

```bash
bob capture 'x wired the lexer'
bob capture 'x1,3 3 fixed the flaky test'
bob capture 'x wired the lexer ='
```

The text after the close token is one Work Log entry, whitespace-normalized
like every capture line, executing byte for byte like its bullet form. With
no number it logs to the first task the close works (task 1 for plain `x`,
the first eligible top-level worked link for wildcard closes). A wildcard's
first eligible link depends on the resolved session lineup; one inline entry
creates one Work Log item, even when the wildcard selects several links. A
leading number names the task and
the rest is the text, so only the first token is an index and
`x 1 3 bugs fixed` logs `3 bugs fixed` to task 1. `0` and leading zeros
report `task numbers start at 1`, overflow reports `task number N is too
large`, and `x0` works no task so its entry has none to log to. The entry
sits right after the close with session operators trailing
(`x wired the lexer =` closes with the entry then starts); a trailing `+1`,
`-`, or `--` with no start or close before it stays text (`x got a +1`).
Entry text follows the bullet rules below and never ends with a Task Link.
A lone number (`x 2`) is incomplete: `capture-parse` needs
`pomodoro_close_log_text` and `bob capture` asks for the text (or `x2`).

##### Several entries on bullets

Log work as child bullets under a whole-item close:

```text
x[<N>][*<P>][!<M>][~<K>]
- [<n>] <entry text…>
  - <detail text…>
- [<m>] <entry text…>
```

```bash
printf 'x\n- 1 wired the lexer\n' | bob capture
printf 'x1,2\n- 1 wired the lexer\n  - chose a hand-rolled lexer\n- 2 sketched the URL parser\n' | bob capture
printf 'x\n- 1 wrote docs\n- 1 opened the PR\n' | bob capture
```

A whole-item close (`x` plus an optional selection, alone on its
parent line, or the `x` of a chain line) takes Work Log bullets as its
child lines, using exactly the authored-bullet rules: a first-level bullet
sits at column 0, a nested one has exactly two spaces, the marker is `-`,
`*`, or `+` plus a space or tab, marker-only rows are harmless
placeholders, and bad indentation or orphaned nested bullets fail with the
existing messages. A first-level bullet is numbered when its first
whitespace-separated token is all ASCII digits: the leading number is always
the task number `<n>`, and everything after it is the entry text. Every
other first-level bullet is unnumbered and its whole body is entry text.
Bullets are numbered all or none: the first first-level bullet fixes the
list's kind. Without wildcard intent, with `<N>` typed (including `x0`) or `*<P>` present,
loggable tasks are the numbers in `<N>`, `*<P>`, or `!<M>`; with no `<N>` or
`*<P>` every number ≥ 1 is loggable except those in `~<K>`. Numbered bullets
and explicitly numbered selections are lexical with no vault access. Wildcard
numbered bullets and positional entries resolve after Bob stages the session
lineup; wildcard inline entries use the first eligible top-level worked Task
Link. Positions under a close without `<N>`/`*<P>` resolve against the running
session at execution.
Only the first token is an index, so `- 1 fixed 3 bugs` logs `fixed 3 bugs`
with no escape, and every backslash is literal. Entries keep typed order,
and the same number may appear on several numbered bullets, each a separate
entry. A two-space nested bullet is a detail of the nearest preceding
entry, numbered or not: it has no task number, so its text is entirely
literal, and it becomes an undated sub-bullet under its dated entry in the
task's Work Log (never counted in `+N Work Log`).

Unnumbered bullets log in order to the close's worked tasks `W`. In
selection mode (`<N>` typed, including `x0`, or a non-empty `*<P>`),
`W` is the lexical `sorted(<N> ∪ *<P> ∪ !<M>)`; otherwise `W` is the running
session's numbered Task Links whose outcome after the selection is in
progress, parked, or complete and that sit at the session's top level
(`[[T]]#` defers, `![[T]]` completes, nested links never take entries). One
worked task takes every bullet; otherwise bullet `i` logs to `W[i]`, so a
single bullet logs to the first worked task, matching the inline default.
More bullets than worked tasks is an error, as is a close that works no
task. For example `x3,4` plus `- foo bar` plus `- baz bam` writes exactly
what `- 3 foo bar` plus `- 4 baz bam` writes; `x2` plus two bullets logs
both to task 2; and plain `x` plus `- wired the lexer` logs to the first
worked top-level link. A leading number is always a task number (`- 2 bugs
fixed` names task 2), so to log text that starts with a number, number every
bullet (`- 3 2 bugs fixed`).

Entry and detail text are literal and whitespace-normalized like every
capture line: `@route`, `@@route`, `s:<N>`, `$<N>`, `%`, `#`, and `:query`
are not markers here, plain wikilinks (`[[Design notes]]`,
`[[note#Heading]]`) are allowed, and the bullets never feed `@@`
declarations, item-wide markers, or `capture-rewrite` absorption. Two
things are rejected in entry and detail text alike because the close would
misread them: any block link or embed (`[[path#^id]]`, `![[path#^id]]`) and
text starting with a code fence (` ``` ` or `~~~`).

The close line holds at most one inline entry, which belongs to the close
it follows. An inline entry plus any child bullet fails with the bullet to
write the inline entry on
(``an inline Work Log entry can't be combined with Work Log bullets; for several entries, put this one on its own bullet too: `- 1 wired the lexer` ``);
details need bullets too. Only the whole-item `x` takes an inline entry or
Work Log bullets; link-form closes keep
their current behavior (`^route:id=x` and solo `@route:id=x` reject
children, `<text> @route:id=x` writes them as the new task's sub-bullets).

Worked example on the fixture above:

```bash
printf 'x1,2\n- 1 wired the lexer\n  - chose a hand-rolled lexer\n- 2 sketched the URL parser\n' | bob capture
```

`- wired the lexer` (with `- chose a hand-rolled lexer` under it) is
appended under link 1 after `Wrote the plan`, `- sketched the URL parser`
under link 2, and `^capture-stop`'s Work Log ends with the dated entry plus
its undated detail. Human output prints typed entries first, all of them,
not dimmed, each followed by its details indented two more spaces; JSON
reports `pomodoro_close.log`
(`[{ "index": 2, "text": "wired the lexer", "details": ["chose a hand-rolled lexer"] }]`,
`details` omitted when empty; in `bob capture` JSON `index` is always the
task the entry logged to, including for unnumbered bullets) and each row's
`typed_work_log` subset plus
the aligned `typed_work_log_details`. Diagnostics (all
`invalid_pomodoro_close`, nothing written): mixed numbering
(``start each Work Log bullet with the number of the task it logs to; bullets are numbered all or none: `- 1 b` ``
for a numbered list with a later unnumbered bullet;
``Work Log bullets are numbered all or none, and the first one has no task number; …``
for an unnumbered list with a later numbered bullet),
too many unnumbered bullets
(`` `x3,4` works 2 tasks (3 and 4) but has 3 unnumbered Work Log bullets; …`` lexically,
or `` `x` works 2 of CAPTURE's tasks (1 and 3) but has 3 unnumbered Work Log bullets; …``
against the running session) and a close that works no task,
a non-loggable index
(``task 3 isn't worked by `x2`; list it (`x2,3`) or complete it (`x2!3`) to log to it``;
``task 2 is dropped by `~2`, so it can't take a Work Log entry``;
`task numbers start at 1`; `task number N is too large`),
a dangling bullet (`` `- 1` is incomplete: type the Work Log text after task 1 ``;
`capture-parse` reports `incomplete` needing `pomodoro_close_log_text`),
a block link
(``a Work Log bullet can't contain the block link `[[bob#^x]]`; the close would treat it as a Task Link``),
and a fence (``a Work Log bullet can't start with a code fence``).

### Chaining session operators on one line

The seven whole-item Pomodoro session operators — `+[N]`, `-[N]`, `++[N]`,
`--[N]`, `=`/`=<X>`, `=name`, and `x[<N>][!<M>][~<K>]` — may share one physical line when
whitespace separates them:

```bash
bob capture '+2 x'
bob capture 'x ='
bob capture 'x =bugs'
bob capture 'x =~2'
bob capture '=~2 +2'
```

Recognition: the line must hold at least two whitespace-separated tokens and
every token must be a session token — one the whole-item session parsers
would claim as a standalone one-line item, near misses included. A single
token is never a chain, so single-token behavior is byte-identical. Otherwise
the lead (maximal session-token prefix) splits at its last whole-item close
(the owner), the trail (maximal session-token suffix trimmed to begin with a
start or close token) runs after, and the entry is every token after the
owner before the trail. A line with no close in its lead is one item (`+2
more`, `=3 more`, and `++3 plan` keep their shape errors, `Plan +2 x` and
`- foo` stay prose). `-2 x wired the lexer` shortens then closes with the
entry, `x wired the lexer =` closes with the entry then starts, and `x got
a +1` keeps `+1` as text. `x 1,3` and `x1 !2` keep the no-spaces hint
because `1,3` and `!2` are not chain tokens.

Tokens run left to right exactly like blank-line items: `+2 x` extends then
closes, `x =` closes then starts the next future Pomodoro (a session switch
in one line), `x =bugs` closes then starts the open `BUGS` session,
`x2 =3` closes keeping task 2 in progress then starts a
15-minute session, `= +2` starts then extends, `=bugs +2` starts `BUGS`
then extends it, `x =~2` closes then starts without link 2 of the next
lineup (while `x~2 =` drops link 2 of the running session and then
starts), `=~2 +2` starts without link 2 then extends, and `--2 +` shifts earlier
then extends. Staging, rollback, and `--dry-run` match blank-line batches:
later tokens see earlier staged edits through `CaptureBatchPlanner`, and any
failure rolls the whole batch back. Output is per token: one JSON/human
result per operator, and `capture-parse` reports one `items[]` entry per
token with per-token ranges sharing the physical line numbers.

Child lines attach to the line's `x` item — or to the last `x` when the
line closes twice — so `x =` plus `- 1 wired the lexer` logs the entry then
starts, and `-2 x` plus bullets shortens then closes. When the line has an
inline entry, children attach to its owner, so the mixing error fires.
With no `x` on the
line, children still attach to the last token's item and fail that token's
existing shape rule, so a chain line with children never becomes a prose
task — including bare-first chains such as `+ x` with a child bullet. A
chain whose `x` is not last nests its item ranges: the close item's range
runs from its token through its last bullet, so it contains the ranges of
the later tokens on the parent line. An inline owner never nests siblings:
its range runs from its token through the entry's end (or its last child).
Item ranges therefore nest or stay disjoint; they never
partially overlap.

Spacing is significant: `= -2` starts a 25-minute session then shortens it
by 10 minutes, while `=-2` is one start with a 10-minute offset; likewise
`=bugs +2` starts `BUGS` then extends it, while `=bugs+2` is one start of
the Pomodoro named `BUGS+2`. Lines made
only of bare operators are newly recognized chains (`- -`, `+ -`, `= =`,
`-- --`, `- - -`); they used to be prose. Runtime guards apply per token in
order (`x -2` closes then fails because nothing is running; `= =` fails on
the second start), and forced `--route`/`--section`/`--task`/`--task-ref`/
`--task-section`/`--clip` fail on the first chain token.

Single-quote the argument: zsh expands a leading `=word`, so write
`bob capture '+2 x'`. Positional args are already joined with spaces, so
`bob capture +2 'x'` also works. Task/link forms, markers, `@@`, and prose
never chain, the JSON contract is unchanged (`schema_version` 1, no new
keys), and no SASE memory changes.

### Project notes

Use a leading or trailing `@<route>^<block-id>+` marker to create a brand-new
sub-project note instead of a task. A `#<pomodoro>` name after the `+`
(`@<route>^<block-id>+#<pomodoro>`) picks the Pomodoro that ` :<id>` Task
Links go under. Task bullets name their IDs with a trailing ` :<id>` (link
into the Pomodoro) or ` ^<id>` (name only). For example:

```text
Finish the Google exit packet! @cash^goog-exit+#admin
- Draft the resignation memo :draft-memo
  - keep it short
- Call Morgan Stanley about the 401k :call-ms
- Collect the equity paperwork ^equity-docs
- FUTURE WORK
  - Revisit the severance terms
```

This creates `cash_goog_exit.md`:

```markdown
---
parent: "[[cash]]"
template: "[[new_project]]"
type: "[[project]]"
status: wip
created: 2026-09-29T14:31:07-0400
---

- [ ] #task #prj Finish the Google exit packet! #hide ^prj

## Tasks

- [*] #task Draft the resignation memo [created::2026-09-29] ^draft-memo
	- keep it short
- [*] #task Call Morgan Stanley about the 401k [created::2026-09-29] ^call-ms
- [ ] #task Collect the equity paperwork [created::2026-09-29] ^equity-docs

## Future Work

- Revisit the severance terms
```

In the same atomic write, it appends two links to the daily note's `ADMIN`
Pomodoro, in source order — creating the future placeholder
`- [ ] () — ADMIN` first when no open `ADMIN` Pomodoro exists:

```markdown
- [ ] () — ADMIN
  - [[cash_goog_exit#^draft-memo]]
  - [[cash_goog_exit#^call-ms]]
```

Without `#admin`, the links go to today's implicit current/next Pomodoro,
the same selection rule `@route:id` uses.

This mirrors the Obsidian Bob Navigation Hotkeys command **Create project
note from task**, so CLI-created notes are indistinguishable from
hotkey-created ones — including the `template: "[[new_project]]"` line, which
the Obsidian flow inherits from `_templates/new_project.md` and never strips.
The CLI renders the note directly and never reads that template.

The `+` sigil sits immediately after the block ID, before any `#` component.
A trailing `+` would be ambiguous — `@sase:deep-fix#bugs+` is a valid
Pomodoro-linked task naming the Pomodoro `BUGS+` — while `+` right after the
block ID takes nothing away, because block IDs accept only letters, digits,
and `-`.

The filename is `<route>_<block-id with every '-' replaced by '_'>.md` at
the vault root. The route arrives lower-cased from the marker parser; the
block ID keeps its authored case. `parent` is `"[[<route>]]"` — every capture
route is a vault-root note, so the `[[path|basename]]` form never applies.
`created` uses the `YYYY-MM-DDTHH:mm:ss±ZZZZ` shape the template's
`tp.file.creation_date` writes, controlled by `BOB_NOW` like every other
capture date.

The `^prj` line is `- [ ] #task #prj <body> #hide ^prj`, where `<body>` is
the item's normalized parent text, never truncated. It is `[ ]` normally and
`[?]` when a scheduled property was resolved; it is never `[*]` and never
linked. `bob task reconcile` reconciles derived Blocked state later, as it
does for every other capture.

`$<N>` writes `[priority::<value>]` inline on the `^prj` line, before
`#hide ^prj`, and a rolled `$<N>` date still writes its
`🗓️ **SCHEDULE LOG**` child under `^prj`. A resolved scheduled date itself
lands as `[scheduled:: YYYY-MM-DD]` on the `^prj` line, with the other task
metadata, and `^prj` keeps capture's documented `s:<N>` `[?]` rule.
`bob projects sync` never writes frontmatter `scheduled`.

Authored child bullets are re-routed into the new note rather than nested
under the captured line. A first-level authored bullet with at least one
nested authored bullet **and** an ALL-CAPS title (letters, digits, spaces,
and `& ' ( ) , . / -`, starting with a letter or digit) becomes a
`## Title Case` section appended at the end of the note, with its nested
bullets copied in verbatim as `- <body>` lines. Title casing lowercases the
body and uppercases the first character of every alphanumeric run, so
`FUTURE WORK` becomes `Future Work`, `NON-GOALS` becomes `Non-Goals`, and
`API DESIGN` becomes `Api Design` without preserving the acronym. Two
sections whose titles normalize equally (trimmed, whitespace-collapsed,
casefolded) merge into one in source order. A first-level bullet ending with
a task ID always takes the task branch instead — naming it states the intent,
so an ALL-CAPS bullet with nested bullets and a ` :<id>` / ` ^<id>` still
renders as a task. Every other first-level authored bullet becomes a
`- [ ] #task <body> [created::YYYY-MM-DD]` line inside `## Tasks`, with its
nested authored bullets rendered one indentation unit beneath it; an authored
checkbox status is preserved, and a bare ALL-CAPS bullet with no nested
bullets stays a task. An authored `TASKS` section merges into the generated
`## Tasks` section instead of adding a duplicate header. With no authored
task children, the note keeps the template's placeholder line
`- [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::YYYY-MM-DD]` under
`## Tasks`.

A task ID token is one whitespace-free token with a `:` or `^` sigil
followed by at least one character, where the first character after the sigil
is an ASCII letter or digit — so `:)`, `:-)`, and `:(` stay prose. It counts
only as the last word of a first-level bullet's text, after that line's
item-wide markers (`s:<N>`, `$<N>`, `%…`, trailing `@route…`) are set aside:
`- Draft memo :draft-memo s:2` works, while `- Draft memo s:2 :draft-memo`
leaves `s:2` as literal text. The token is stripped from the rendered body
and appended as the last token of the rendered task, after `[created::…]`.
A `:` task renders as `[*]`, or `[?]` when the project resolved a scheduled
date, and gets its `[[<stem>#^<id>]]` Task Link — unless that date is in the
future, in which case the note is blocked (§13.3): the task renders `[?]`
plus `#prj_is_blocked` and stays unlinked, with a `not_linked`
(`project_schedule`) report; a `^` task keeps an authored checkbox status,
or `[ ]` without one. The ID must satisfy the shared
block-ID rule (`A-Z`, `a-z`, `0-9`, `-`); `prj` in any letter case is
reserved. A bare trailing `:` or `^` is an unfinished ID and fails with a
message naming the ` :<block-id>` / ` ^<block-id>` spelling.

A project note with at least one ` :` task reads the daily note and needs a
`## Pomodoros` section; with none, the daily note is never read. Missing
file, no eligible Pomodoro, multiple open timed entries, and "ledger already
contains `[[…]]`" errors reuse the existing Pomodoro-task messages. The note
and the daily note are staged together, so any failure writes nothing, and
later batch items see the staged daily note. Only one named Pomodoro may be
created per item, and every link lands under it in source order.

A `#pomodoro` name with no ` :` task is rejected: `` `#admin` picks the
Pomodoro for ` :<id>` task links, but no task bullet ends with ` :<id>`; add
one or remove `#admin` ``. A task ID anywhere else in a project-note item —
on the parent line, or on a nested bullet — is rejected as misplaced: the
project's own task is always `^prj` and is never linked. Other violations
fail before any write: an invalid or reserved ID (`` task ID `draft_memo`
may use only A-Z, a-z, 0-9 or '-' ``), two IDs that compare equal in one item
(`` task ID `x` is already used on line 2; rename the task on line 4 ``), an
empty remaining body, or an authored checkbox on a `:` task (`` `:foo` makes
the task Next and links it, so it takes no `[x]` checkbox; remove the
checkbox or write ` ^foo` to keep it ``).

Retired forms fail with a message that teaches the new spelling. `@cash:goog-exit+` →
`` `@cash:goog-exit+` is retired: a project note never links its own `^prj`
task. Write `@cash^goog-exit+` and end each task bullet you want in the
Pomodoro with ` :<id>` `` (with `#bugs`, the suggestion is
`@cash^goog-exit+#bugs`). A misordered `+` also teaches the fix:
`@cash^goog-exit#bugs+` →
`` put the project-note `+` right after the block ID:
`@cash^goog-exit+#bugs` (a `+` after `#bugs` would be part of the Pomodoro
name) ``, and `@cash^goog-exit#bugs` →
`` `#bugs` after `@cash^goog-exit` needs the project-note `+`
(`@cash^goog-exit+#bugs`); to link a task under a Pomodoro, use
`@cash:goog-exit#bugs` ``.

The parent note `<route>.md` must already exist and be an area or
non-terminal project note — the same rule `bob capture-targets` lists routes
by. A missing parent fails with
`cannot create a project note under <route>.md: note does not exist (run 'bob capture-targets' to list routable notes)`;
a parent that is neither area nor project fails with
`... note is not an area or project note`, and a done or canceled project
fails with `... note is a <status> project`. Parent reads see earlier batch
items, so a later item can parent onto a note an earlier item created. When
`<route>_<id>.md` already exists on disk or was already staged earlier in the
same batch, capture fails with `project note already exists: <path>` and
writes nothing. The parent note itself is never modified: `bob projects sync`
owns the generated `- 🧩 **Sub-projects:** [[...]]` line, which is
machine-owned and rewritten there.

Project-note capture cannot be combined with clipboard input:
`%...` markers fail with
`project-note capture cannot be combined with % clipboard markers` and
`--clip` with `project-note capture cannot be combined with --clip`.
Authored children are re-routed into `## Tasks` and `##` sections, so
clipboard children under the captured parent have no unambiguous home; this
is a deliberate deferral a future change can lift. Forced destination flags
(`--route`, `--section`, `--task`, `--task-ref`, `--task-section`) keep
`@tokens` literal, so they never reach this family. `s:<N>` and `$<N>` are
allowed and behave as described above. A `=<X>` / `=x` suffix is rejected on
project notes with a message naming `@<route>^<block-id>+`; `!` stays
reserved for the sub-bullet toggle and is not accepted on this family; a
`@@<route>^<id>+` declaration fails as `invalid_global_destination`, since
one declaration creating the same note per item is never what was meant; and
an empty block ID (`@<route>^+`) fails with the family's existing "requires
a block ID" wording.

### Sub-bullets under existing tasks

Use a leading or trailing `@<route>+<block-id>` marker to capture an ordinary
child bullet beneath an existing task, without creating a note or changing the
parent task. For example,
`bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'` writes:

```markdown
- [*] #task Finish Google Exit Packet! [created::2026-07-31] ^goog-exit
  - Called Morgan Stanley today.
```

When the selected task already has a direct-child Schedule Log, Work Log, or
Rotten Log, the complete new child — including any authored children, clipboard
children, or a `$<N>`-generated Schedule Log nested under that child — is
inserted immediately before the earliest of those managed logs. A Schedule Log is the
`🗓️ **SCHEDULE LOG**` child that records schedule changes; a Work Log is the
`🛠️ **WORK LOG**` child that records work summaries; a Rotten Log is the
`🥀 **ROTTEN LOG**` child that records keep history. Nested or lookalike log
markers do not move the insertion point. Tasks with neither managed log still
append at the end of the task block.

The marker composes with terminal `s:<N>`, `$<N>`, and clipboard markers in
either order. Scheduled properties are still rendered for consistency even
though Obsidian Tasks does not read them from an ordinary bullet. Existing child
indentation is copied; otherwise capture uses the note's dominant tab-or-two-space
indentation and falls back to a tab. Line endings are preserved. The note and
task must
already exist, block IDs must be unique, and non-task block IDs are rejected.
Missing IDs include a close-match suggestion when possible and direct callers
to `bob capture-tasks -r <route>`.

The same marker accepts an optional trailing `#<section>` selector, as in
`@cash+goog-exit#requirements` or `@foo+bar#future-work`. The selector names an
ALL-CAPS child section of that task and may use A-Z, a-z, 0-9, and
`& ' ( ) , . / -`. Whole-slug matches beat earlier prefix matches, so
`#future-work` still reaches `FUTURE WORK` even when `FUTURE WORKFLOW` appears
first; `#future` reaches the first slug that starts with `future`. The captured
block is appended at the end of that section's own block — before the next
direct child of the parent task, and before a managed log nested under the
section — using the section's child indentation. A selector that matches
nothing, or a task with no sections, is an error listing the real titles and
pointing at `bob capture-task-sections`; capture never falls back to the end of
the task. `@route+id#` with an empty selector is incomplete and reports need
`task_section`; it does not mean "any section". A second `#`, or `#` before
`+`, is not this family (`@foo#bar+baz` remains a note-bullet).

For example, `bob capture 'Postgres 17 minimum @foo+bar#requirements'` against

```markdown
- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar
	- REQUIREMENTS
		- existing
	- FUTURE WORK
```

appends the new bullet inside `REQUIREMENTS` rather than at the end of the
task:

```markdown
- [ ] #task Upgrade Postgres [created::2026-07-31] ^bar
	- REQUIREMENTS
		- existing
		- Postgres 17 minimum
	- FUTURE WORK
```

Every sub-bullet batch also reports its parent tasks once, in final state,
in the batch-level `task_blocks` array; see
[Task blocks](#task-blocks).

### Task Link toggle

A capture item that is exactly `@route+block-id`, with no body text and no
authored child bullets, updates that existing task instead of writing a child
bullet. `bob capture '@cash+goog-exit'` is the default Ensure Next operation:
it makes the task Next and relocates its existing Task Link. The route note and
daily note are planned together before either file is written, so a failure
leaves the whole batch unchanged.

`@route+block-id#pomodoro` is the same Ensure Next operation with a Pomodoro
selector. The `#` component follows the item's mode: with no body text it
selects a Pomodoro name, using the same slug matching, canonicalization, and
named-future-entry creation rules as `@route:block-id#pomodoro`; once the item
has body text, `#section` keeps its child-bullet meaning and selects an
ALL-CAPS task section. Whole-slug matches beat prefix matches. A valid selector
with no matching open entry, including a completed-only match, creates the
canonical named future Pomodoro and then moves the existing Task Link subtree
into it. Ensure Next never creates a missing Task Link; that remains an
explicit reason to use `@route+block-id!`.

The link-presence toggle, including Pomodoro-link insertion and
all-open-link removal, is reserved for the terminal `@route+block-id!`
spelling. `!` cannot be combined with `#pomodoro`. The ledger decides the
direction: no matching link under an open Pomodoro links the task, and any
matching link unlinks it. No toggle path ever lowers a lane.

Link presence for `@route+block-id!` is:

| Linked under an open Pomodoro today? | Status | Result |
| --- | --- | --- |
| no | Ready `[ ]` / Blocked `[?]` | Next `[*]`, with `[[route#^block-id]]` linked under the implicit current/next open Pomodoro — unless the §13.5 link guard consumes it; §13.5.1 first drops its own open prerequisites when every safety gate passes |
| no | Next `[*]` / In Progress `[/]` | Link inserted under the implicit current/next open Pomodoro; status unchanged |
| yes | any open status | Every matching link under every open Pomodoro removed; status unchanged |
| — | done, canceled, or unknown | Error: `task ^<id> is <Status Name>; only Ready, Blocked, Next, and In Progress tasks can be toggled` |

When an unlinked task links with `!`, Bob inserts the task
link under the implicit current/next open Pomodoro unless that entry already
has the same link. In that idempotent case no duplicate is written and JSON
reports `pomodoro_already_linked: true`. Matching duplicate links under later
still-open Pomodoros are removed. Completed Pomodoros are not cleaned up.
When a linked task unlinks, every matching link under every open Pomodoro is
removed and the route note is left untouched. Neither unsuffixed marker-only
form can enter that insertion or removal cleanup.

If the task line has exactly one valid future `[scheduled::YYYY-MM-DD]` field
and the link direction sets the task to Next, Bob removes that scheduled field.
Unlinking never touches the task line, so it never retires a schedule. When
the task already owns a direct-child `🗓️ **SCHEDULE LOG**`, Bob prepends a
dated entry under it:

```markdown
- _2026-07-20 → 2026-07-10_ — 🍅 pulled into today's Pomodoro
```

No Schedule Log is created for a task that does not already have one, and past,
today, invalid, or multiple scheduled fields are left alone. When the link
direction rewrites the task line, Bob stamps `[fresh:: TODAY]` before the
trailing Tasks fields in the same write (see `docs/freshness.md`); unlinking
never touches the task line and never stamps, and a Next or In Progress link
that retires no future schedule leaves the note untouched.

Task lookup errors match sub-bullet capture: unresolved route notes, missing
block IDs, duplicate block IDs, non-task block IDs, close-match suggestions,
and the `bob capture-tasks -r <route>` hint are reused. Daily-ledger errors
match Pomodoro-linked capture: `Bob daily note does not exist: <path>`, `Bob
daily note has no Pomodoros section`, `Bob daily note has no eligible open
Pomodoro`, `Bob daily note has multiple open timed Pomodoros`, and invalid
Pomodoro names with ``Pomodoro name must contain only A-Z, 0-9 or `& ' ( ) + , . / -` and must start with a letter or digit``. A task toggle also
rejects forced destination flags with `task toggle capture cannot be combined
with <flags>`, `--clip` with `task toggle capture cannot be combined with
--clip`, `%...` with `task toggle capture cannot be combined with % clipboard
markers`, `s:<N>` with `task toggle capture cannot be combined with s:<N>`,
`$<N>` with `task toggle capture cannot be combined with $<N>`, and authored
child bullets with `task toggle capture cannot have authored child bullets`.

The marker-only `@route+block-id` and `@route+block-id#pomodoro` forms are
Ensure Next operations. Ready `[ ]`, Blocked `[?]`, In Progress `[/]`, and
Next `[*]` are eligible: Ready and a task Blocked only by its own
prerequisites rise to Next after a §13.5.1 drop, while Next and In
Progress keep their lane. Other Blocked targets stay unlinked (see the
link-guard paragraph below). Like the `!` link direction, Ensure Next stamps
`[fresh:: TODAY]` whenever it rewrites the task line (see `docs/freshness.md`). Done, canceled, unknown, missing,
non-task, and duplicate-ID targets keep actionable errors. This default
reverses the initial `!` implementation: use the unsuffixed forms for
idempotent relocation, and use `@route+block-id!` only when you intentionally
want to toggle the link itself — add a missing link, or remove every
open-Pomodoro link with the lane unchanged.

The plain form's destination is today's implicit current/next open Pomodoro:
the single open timed entry when present, otherwise the first open entry in
document order. The named form's destination is the matching open named
Pomodoro, or a newly created named future Pomodoro when no open name matches.
A missing Pomodoros section, no eligible open implicit entry, multiple open
timed entries for implicit selection or named creation, or an invalid
Pomodoro name is an atomic error. Existing named selection skips the
multiple-open-timed guard. The command never creates a missing Task Link; if
no dedicated Task Link exists under an open Pomodoro, it fails write-free and
tells the user to use `@route+block-id!` to add one. More than one movable
occurrence is also a write-free invariant error. Completed Pomodoros are
historical and are never edited. A link embedded in surrounding prose is not
a dedicated Task Link.

When the sole link is already under the selected destination, the daily file
is left byte-for-byte unchanged. The task-side plan is independent: a Ready
or Blocked status still becomes Next and a single future schedule is still
retired when an existing Schedule Log is present — unless the §13.5 link
guard finds the target Blocked. If §13.5.1 can safely drop its own open
prerequisites, Ensure Next proceeds and reports them; otherwise the task line
stays unchanged, any existing live links to that task are removed from today's
open ledger, the open prerequisites rise, and the result reports
`link_guard` (human output prints `stays <Status>` plus the guard sentence).
An already-Next task whose
link is already at the destination is a true no-op. When relocation is
needed, Bob moves the dedicated link bullet and its complete descendant
subtree, adapting only the root indentation to the destination's established
child indentation.

Human output says `would ensure` / `ensured` rather than `would toggle` /
`toggled`, distinguishes "set Next" from "already Next" and "stays In
Progress", and prints either
the source-to-destination Pomodoro move (naming both Pomodoros, and naming a
created destination) or `Task Link already in <name>; no ledger change.` /
`Task Link already in current/next Pomodoro; no ledger change.` JSON stays
schema version 1 and kind `"task_toggle"` with `toggle_direction: "next"`.
Additive fields let new clients render the outcome precisely while old
clients ignore them:

- `toggle_behavior: "ensure_next"` (omitted for the `!` toggle)
- `status_changed: true|false`
- `pomodoro_link_action: "moved"|"already_current"`
- `pomodoro_link_source` and `pomodoro_link_destination` objects with
  one-based `line` plus optional `name` and `time_range`
- `pomodoro_name` set to the resolved canonical destination name
- `creates_pomodoro: true` only when this operation created the named entry
- `pomodoro_already_linked: true` only for the already-at-destination no-op
- `removed_pomodoro_links: 0` with no `pomodoro_selector_unused` clearing story

A terminal `!` on the same marker-only shape, `@route+block-id!`, opts into
the link-presence toggle. It is accepted only as the final byte of that
exact marker: not on `@route+id#name`, `@@route+id`, body-bearing items,
authored children, clipboard, schedule, priority, or forced destination flags.
Ordinary `!` in capture prose stays literal. An unlinked Ready `[ ]` task,
or a Blocked `[?]` task whose own open prerequisites §13.5.1 can drop,
becomes Next `[*]` and uses the existing link-insertion path; other Blocked
targets stay unlinked. An unlinked Next `[*]` or In Progress `[/]` task
links with its status unchanged; a linked task of any open status unlinks
with its status unchanged.
Done, canceled, and unknown states keep the normal toggle errors.

Human output says `would toggle` / `toggled` and prints a link summary:
`linked ^id under NAME · set Next`, `linked ^id under NAME · stays Next`,
`linked ^id under NAME · stays In Progress`, `not linked ^id · stays
<Status>` plus the guard sentence when consumption replaced the link, or
`unlinked ^id from N open Pomodoros · stays <Status>`, followed by the day
file and ledger lines. A successful dependency drop adds a dim
`⛓️ dropped dependency on [[route#^id]]` line after the status line (or
`⛓️ dropped N dependencies: …` for several). JSON stays schema version 1 and kind `"task_toggle"`.
Successful link and unlink actions include `toggle_direction: "link"` or
`"unlink"`, `status_changed`, `removed_pomodoro_links` for unlink, and the
existing insertion fields for link. A consumed `@route+block-id!` keeps kind
`"task_toggle"` but omits `toggle_direction`, `block_link`,
`creates_pomodoro`, `pomodoro_already_linked`, `pomodoro_selector_unused`,
`pomodoro_link_placement`, `pomodoro_name`, and `pomodoro_link_action` plus
its source/destination endpoints. It reports `link_guard` (`reasons`,
`promoted`, `detail`, and `drop_refused` when an own dependency reason kept
the prerequisites); successful drops report `dropped_dependencies` in line
order and stage the dependent's removal from each dropped prerequisite's
BLOCKS line (`blocks_line_updates`). `removed_pomodoro_links` remains present, and
`day_file` is present only when cleanup removed at least one live link. A
`^prj` whose only reason is its own future schedule is refused with "move
the project date from the `^prj` Schedule action".

Compatibility fields: `pomodoro_name` is the resolved destination name when
one exists, `creates_pomodoro` is `true` only when this Ensure Next created
the named destination, `pomodoro_already_linked` is `true` only for the
already-current outcome, and destination placement is reported only for an
actual move. Relocation is never described as later duplicate removal through
`removed_pomodoro_links`.

### Pomodoro notes

Append a bare trailing `#` marker to capture the item as a plain-text
sub-bullet on a Pomodoro instead of a task. For example,
`bob capture 'remembered to bump the timeout #'` writes:

```markdown
- remembered to bump the timeout
```

as a child of the selected Pomodoro. It renders as `- <text>` with no
`[created::YYYY-MM-DD]` stamp, no `#task` marker, and no block ID. The daily
note file is selected the same way as `@<route>:<block-id>` captures:
`BOB_DAY_FILE` when set, otherwise `<bob-dir>/YYYY/YYYYMMDD.md`. Unlike
`@<route>:<block-id>`, a Pomodoro note may attach to a completed entry.
Capture prefers the single open top-level entry with a recognized time
range, otherwise the last completed top-level entry, and otherwise the
first open top-level entry. A ledger with only completed entries therefore
succeeds and attaches to the last one. Multiple open timed entries are
an invariant error. The new bullet is appended at the end of the
selected entry's child block, reusing existing child indentation when
possible.

The marker composes with `%...` and `--clip` in either terminal order, since
"capture what I just copied onto this Pomodoro" is a plausible use, but it is
rejected alongside `s:<N>`, `$<N>`, any `@route` token, and `--route`, since
a plain Pomodoro bullet has no field for a schedule, priority, or routed
destination:

| Marker                                                         | With `#` |
| ---------------------------------------------------------------| -------- |
| `%`, `%<N>`, `%<header>`, `--clip[=HEADER]`                     | allowed  |
| `s:<N>`                                                         | rejected |
| `$<N>`                                                         | rejected |
| `@route`, `@route#Sec`, `@route:id`, `@route:id#name`, `@route^id`, `@route+id`, `@route+id#sec` | rejected |
| `--route` / `--section` / `--task` / `--task-ref` / `--task-section` | rejected |

Only a trailing bare `#` is recognized; a leading `#` or a `#` in the middle
of the body stays literal text, and `#<section-prefix>` keeps its existing
meaning as a bullet-section marker (see below) rather than a Pomodoro note.

### Section bullets

Append `#<section-prefix>` or a bare `#` to an `@route` token, as in
`@notes#Ideas` or `@notes#`, to capture an ordinary Markdown bullet instead of
a task. It renders as `- <text> [created::YYYY-MM-DD]` and is placed in a
non-`Tasks` section whose heading title starts with the prefix (compared case
insensitively), or any non-`Tasks` section when the marker is a bare `#`. The
`Next & In Progress`, `Blocked`, and `Done & Canceled` status-group headings
`bob task reconcile` generates under a `Tasks` heading belong to that Tasks
section. They are never bullet targets, so `@note#` and `@note#Bl` skip them. A
matching non-H1 section is preferred; a matching H1 heading is used only when no
non-H1 heading matches. If no heading matches, the bullet goes into the
pre-heading (zeroth) section. When the destination note is an article
(`type: "[[article]]"`), selectors also match on heading identity: the title
with bob's section-number token removed, compared case-insensitively with
whitespace collapsed, and with emphasis, inline code, and wikilink markup
stripped. A prefix selector matches when it is a prefix of that identity, so
`@sase_blog_contents#agent` still finds `## 1.3 Agent CLIs` after numbering. An
exact `--section` title matches when its de-numbered identity equals the
heading's identity, so both `--section "Agent CLIs"` and a stale
`--section "1.4 Agent CLIs"` land on `1.3 Agent CLIs`. Raw title matching still
runs first, so `@note#1.3` prefers `## 1.3 Agent CLIs` over `### 1.3.1 Nested`
in document order. Notes that are not articles keep the raw-title behavior
only. Within the chosen section the bullet is inserted after the last existing
top-level bullet, otherwise just below the heading (or after any YAML
frontmatter for the zeroth section). The suffixed route token may
lead or trail the body, so `@notes#Ideas jot idea` and `jot idea @notes#Ideas`
both capture into `notes.md`. A standalone terminal `#<section-prefix>`
marker not appended to an `@route` token, such as `note #Ideas @foo`, is
still not accepted and fails with a usage error; a standalone bare `#`, such
as `note @foo #`, is instead the Pomodoro-note marker described above and
still fails, since it conflicts with the `@route` token on the same item.

A `--route` target keeps `@tokens` literal. Add `--section TITLE` with
`--route` to force bullet mode and place the bullet in a non-`Tasks` heading
whose title matches `TITLE` exactly, compared case insensitively. Generated
status-group headings under a `Tasks` heading are skipped here too. If no heading
matches, the bullet goes into the pre-heading (zeroth) section — the same
fallback typed `@route#prefix` uses when nothing matches. This exact
section path is intended for picker integrations; typed `@route#prefix` tokens
keep the prefix-matching behavior described above. Without `--section`,
`--route` captures a task.

With `--route`, `-t, --task BLOCK-ID` selects sub-bullet mode while keeping
every `@token` in the text literal. Add `-S, --task-section TITLE` to nest the
new bullet under an ALL-CAPS child section of that task whose title matches
`TITLE` exactly, compared case insensitively. `--task-section` also works with
the hidden `--task-ref` option below; the command requires `--route` and
either `--task` or `--task-ref`. This exact path is the picker
counterpart to `--section`; typed `@route+id#prefix` tokens keep the
slug/prefix matching described above, so `--task-section future-work` does
**not** match `FUTURE WORK` while `--task-section "Future Work"` does. Picker
integrations may instead use the hidden `--task-ref <line>:<digest>` option,
which also reaches parents without block IDs and recovers when unrelated edits
shift the selected task's line.

### Command-line options

Useful options:

- `-b, --bob-dir DIR`: Bob vault root; defaults to `BOB_DIR` or `~/bob`
- `-c, --clip[=HEADER]`: force clipboard capture, optionally with a header
- `-d, --dry-run`: plan and report without writing notes or clipboard files
- `-E, --expand`: print a template draft's expansion and exit before vault
  access or planning
- `-f, --format human|json`: human confirmation or stable JSON for callers
- `-i, --inbox-route ANSWER`: route an inbox task this capture changes:
  `<N|KEY>=<NOTE>` moves it, `<N|KEY>=` keeps it in its inbox, `keep` keeps
  every otherwise unanswered candidate; repeatable. See
  [Routing inbox tasks out](#routing-inbox-tasks-out)
- `-n, --no-clip`: keep trailing `%...` clipboard markers literal
- `-R, --no-ref`: keep bare links as inbox tasks instead of queueing a
  background reference job
- `-p, --pull-in PICKS`: select successor offers to link (`all`, `none`,
  or a comma list of offer numbers and/or keys; see
  [task-dependencies.md §12.14](task-dependencies.md#1214-cli-selection)).
  Hidden `--require-pull-in-choice` (no short alias, absent from
  `--help`) refuses write-free with `code: pull_in_choice_required`
  when offers exist and no `--pull-in` was given
- `--refuse-override`: hidden guard (absent from `--help`) that refuses a
  whole-item restart or swap with `code: pomodoro_override_refused`; Bob Mac
  Capture sends it when the matching on-screen preview has not shown the
  override warning. Example refusal:

  ```json
  {"code":"pomodoro_override_refused","error":"capture item 1 starting on line 1: refusing capture: `=` would restart the running CAPTURE (0920-0945) and --refuse-override is set; capture `x =` to close it first","ok":false}
  ```
- `-r, --route NAME`: force `NAME.md` and keep any `@tokens` in the text literal
- `-s, --section TITLE`: with `--route`, force a bullet into the exact section
- `-t, --task BLOCK-ID`: with `--route`, append beneath the identified task
- `--task-ref LINE:DIGEST`: hidden picker option; with `--route`, append beneath
  the task identified by that stale-safe ref from `capture-tasks` or
  `capture-complete`. Conflicts with `--task`. Not listed in `--help`.
- `-S, --task-section TITLE`: with `--route` and `--task` or `--task-ref`, nest
  under the exact ALL-CAPS child section; conflicts with `--section`

### Input, stdin, and JSON output

If `TEXT` is omitted and stdin is piped, `bob capture` reads the complete
piped stdin stream, so a multi-line authored-bullet draft survives a pipe:
`printf 'parent\n- child\n' | bob capture`. Put options before text, or use
`--` when the task itself starts with a hyphen; a multi-line draft passed as
an argument needs its own shell quoting, for example
`bob capture -- "$(printf 'parent\n- child\n')"`. Before draft text begins,
an unknown alphabetic `--flag` is rejected with exit 2
(`unexpected argument '--flag' found`, JSON `ok: false` under `-f json`)
and writes nothing. After `--` or an earlier text argument it stays literal:
`bob capture -- --anything` and `bob capture hello --world` both capture text.
`--` plus a non-letter (such as `--2`) and a bare `-` also begin
text. Embedded newlines inside a
single `TEXT` argument stay intact; multiple `TEXT` arguments are still
joined with single spaces, never newlines. Editor clients such as Bob Mac Capture should
call `bob capture --format json -- <text>` and parse the JSON object, whose
stable fields include `ok`, `dry_run`, `routed`, `route`, `route_label`,
`relative_target`, `target`, `text`, `task_line`, `kind`, `created`,
`placement`, `open_notes`, `pull_in`, and `inbox_route` (`supported: true`
on every success; see [Routing inbox tasks out](#routing-inbox-tasks-out)).
`open_notes` is a batch-level array on every success (dry-run and real run
alike); see [Open notes](#open-notes). The
template path additionally returns `template: {line, field_count, row_count}`
and `template_row: {row, line}` on each expanded item, including the legacy
top-level result for row 1. A successful real template capture also saves the
block to the shelf (see [Saved templates](#saved-templates)): the `template`
object then gains `saved: {id, status}` (`new`, `updated`, or `replaced`),
or `not_saved: "<reason>"` when the best-effort save failed. Both are absent
on `--dry-run` and when saving is off. Human output appends `· saved`
(`· saved, replacing <origin-id>` for `replaced`, naming the checked-out
origin that was removed) or `· not saved` to the template summary line. `-E, --expand` prints the expanded draft in human
mode; with `-f json` it returns `expanded` and the same `template` summary. On
drafts without a header it returns the original text. The
`kind` field is `"task"`, `"bullet"`, `"pomodoro_task"`,
`"pomodoro_note"`, `"sub_bullet"`, `"task_toggle"`, `"project_note"`,
`"pomodoro_adjust"`, `"pomodoro_shift"`, `"pomodoro_start"`,
`"pomodoro_close"`, `"pomodoro_link"`, `"task_dependency"`, `"task_complete"`, or `"ref"`, and
`task_line` holds the rendered line for any kind — for `"project_note"` it is
the rendered `^prj` line, and for `"pomodoro_link"` it is the linked task's
post-image line. On JSON-mode failures, stdout is still a
single object with `ok: false` and an `error` string. Template framing and
refusal errors include their stable `template_*` code. `code` also appears on
`plan_theme_cap_exceeded`, `pull_in_choice_required`,
`inbox_route_choice_required`, `inbox_route_stale`, and `inbox_route_refused`
refusals.

A reference item (`kind: "ref"`) uses `placement: "queued"` or
`"unchanged"`, leaves `task_line` empty, and carries the additive `ref`
object: `url`, `cleaned_url`, `dedupe_key`, `display`, `route_hint`
(`"article"`, `"pdf"`, or `"arxiv"`), `library` (`verdict` plus the known
`path`, `title`, `reading_state`, or `message`), `job` (`id` and `state`,
only on a real run that queued), and `fallback` (`relative_target` and the
exact `task_line` the failed clip writes, only when queued). A dry run's
output equals the real run's apart from `dry_run` and `job`.

A capture with authored sub-bullets additionally includes a `sub_bullets`
array of the exact rendered child lines, including their target-selected
indentation, in source order; it is omitted entirely for an ordinary
capture with no authored children. A project-note capture always omits
`sub_bullets` — authored children are re-routed into the new note's
`## Tasks` and `##` sections, and the `project_note` summary below replaces
it. Human output prints those lines directly
beneath `task_line`, before any clipboard children and schedule log.

A `$<N>` capture additionally includes `priority` (the written value, such as
`"high"`) and `priority_label` (the configured label, such as `"P1"`); a
capture without `$<N>` omits both fields.

A `$<N>` capture that actually rolled the scheduled date additionally
includes a `schedule_log` object: `reason` (the `🎲 …` text) and `lines` (the
exact rendered `🗓️ **SCHEDULE LOG**` marker and entry lines, in note order).
The schema is unchanged; the reason text records the exact selected day count
in bold and the configured range in parentheses. `schedule_log` is omitted
when `$<N>` was not given, or when `s:<N>` won the scheduled date and no roll
happened.

Clipboard captures additionally include a `clip` object. Single captures keep
the existing shape: `header`, `mode` (`"inline"`, `"lines"`, `"attachments"`,
or `"snippet"`), `lines` (the exact rendered child lines), `attachments`, and
`entries`. Leaf clips emit `entries: []`. Each attachment has `source`,
vault-relative `saved`, `kind` (`"image"` or `"file"`), and `reused` fields.
Snippet results also include the vault-relative `snippet` path. The `header`
value is `null` when the capture omitted a header and is the rendered string
(for example, `"BUILD LOG"`) when one was explicit.

Counted histories above one use `mode: "history"`, `header: null`, flattened
`lines`, and attachment records aggregated in entry order. Their `entries`
array contains one ordinary headerless clip object per requested value, keeping
entry boundaries and any owning `snippet` path explicit. The aggregate omits a
singular `snippet` field. `%1` uses the unchanged single-capture shape.
`task_line` remains the parent line only, and non-clipboard JSON omits `clip`.

ID-only task results use kind `"task"` and additionally include `block_id`.
They omit `day_file`, `block_link`, and `pomodoro_link_placement`.

Pomodoro-linked results use kind `"pomodoro_task"` and additionally include
`block_id`, `day_file`, `block_link`, `pomodoro_link_placement`,
`pomodoro_name` (the resolved destination name), `creates_pomodoro`,
and `pomodoro_link_destination` (with its `role`; see
[Plan budget and strict mode](#plan-budget-and-strict-mode)). The touched
Pomodoro also appears in the batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks).

Successful solo-link results use kind `"pomodoro_link"` with `placement: "linked"`,
`routed: true`, and `text: ""`. They carry the post-image `task_line`, the
`block_id`, `day_file`, `block_link`, `previous_task_line`,
`previous_status_symbol` / `previous_status_name`, `status_symbol` /
`status_name`, `status_changed`, `removed_scheduled`, `schedule_log` (when
written), `pomodoro_name`, and `creates_pomodoro` fields; they add
`pomodoro_link_action` (`"linked"`, `"moved"`, or `"already_current"`),
pre-image `pomodoro_link_source` (only when a link existed), post-image
`pomodoro_link_destination` (with its `role`; see
[Plan budget and strict mode](#plan-budget-and-strict-mode)),
`pomodoro_link_placement` (when a link was
inserted or moved), and `pomodoro_start` (only with `=<X>`). They emit no
`toggle_direction` / `toggle_behavior`, so older clients degrade to a neutral
preview instead of a wrong one. Every touched Pomodoro also appears in the
batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks).
When the guard consumes `@route:id`, the result uses kind `"link_guard"`
and omits `block_link`, `creates_pomodoro`, `pomodoro_link_action` and its
source/destination endpoints, `pomodoro_link_placement`, and
`pomodoro_name`. It includes additive `removed_pomodoro_links` and `day_file`
only when existing live links were removed. The `link_guard` object carries
the outcome; when a local dependency reason prevents a drop, it also carries
`drop_refused` (`project_blocked`, `project_task`, `schedule`, or
`dependency_line`) and `detail` says why the prerequisites were kept. Any
existing live-link cleanup also appears in `pomodoro_blocks`.

A successful dependency drop keeps kind `"pomodoro_link"` and adds
`dropped_dependencies` in Depends-On line order. Each row has the prerequisite
`note_path`, `block_id`, exact `link`, clean `text`, `status_symbol`, and
`status_name`. The same staged batch removes the dependent from each dropped
prerequisite's `🧱 **BLOCKS:**` line (`blocks_line_updates`; see
[task-dependencies.md §14](task-dependencies.md#14-blocks-lines)). Human
output prints `⛓️ dropped dependency on [[route#^id]]`
or `⛓️ dropped N dependencies: …` after the status line (`would drop` in a
dry run). The batch-level `task_blocks` entry for the dependent includes role
`"dependency_dropped"` and shows the changed task and Depends-On lines.

Relation captures keep kind `"task_dependency"` and add, per item,
`relation_updates[]` (`{direction, source, target, label,
previous_label, action: added|relabeled|unchanged}`),
`linked_from_line_updates[]`, `minted_relation_block_ids[]`, and
`mirror_skipped[]` (`{reason:
archived_target|previous_daily_target|malformed_line|disabled}`), all
omitted when empty. An existing task whose LINKS line changed is
reported as a task block with role `relation_source`; mirror-only
changes report rows, not task blocks. Human output prints the `🪢`
rows shown in [Linking tasks with '~' and
'~~'](#linking-tasks).

Project-note results use kind `"project_note"` with `placement: "created"`.
`route` stays the parent route (`cash` for `@cash^goog-exit+`), so
route-based clients keep working, while `relative_target` and `target` point
at the new project note (`cash_goog_exit.md`). `route_label` is the written
file name (`cash_goog_exit.md`) — the one capture kind where `route_label`
is not `<route>.md`, because the project note is what a notification should
show as the destination. `block_id` is `"prj"`, and `task_line` is the
rendered `^prj` line. A new `project_note` object reports `basename`,
`parent_route`, `parent_link` (for example `"[[cash]]"`), `tasks` (the count
of `## Tasks` lines written, including the placeholder when it is kept),
`sections` (the ordered array of rendered section titles, excluding
`## Tasks`), and `task_links` — always present, possibly empty — with one
`{"block_id", "block_link", "text", "task_line"}` entry per `:` task in
source order. `block_link` is `[[<stem>#^<id>]]` and `task_line` is the
rendered `[*]`/`[?]` task line. When at least one link was written, the
result also reports the top-level `day_file`, `pomodoro_link_placement`
(from the first insertion), `pomodoro_name` (canonical, when named), and
`creates_pomodoro` fields with their `pomodoro_task` meanings.
Project-note results omit the top-level `block_link`, which used to be the
`^prj` link; `block_id` stays `"prj"`. `scheduled`, `priority`,
`priority_label`, and `schedule_log` keep their existing meanings, and
`sub_bullets` is omitted as described above. Human output prints the
created note, its `parent` link, the `^prj` line, a `<N> <task|tasks> ·
sections <titles>` line, then — when links were written — `✓ linked
<day_file>` (dry-run: `would link`), an `under <NAME>` line with
` (created)` when a named Pomodoro was created, one dim `- [[stem#^id]]`
line per link, and the hint that `bob projects sync` adds the parent's
Sub-projects line and `bob task reconcile` reconciles Blocked state, since
neither is written here. When links were written, their Pomodoro also appears
in the batch-level `pomodoro_blocks` array; see
[Pomodoro blocks](#pomodoro-blocks).

Sub-bullet results additionally include `parent_line`, `parent_text`,
`parent_status_symbol`, and `parent_status_name`. A capture that targeted a
task section also includes `parent_section` (the matched original title);
plain `@route+block-id` captures omit it. They reuse `block_id` for the
parent's ID, omitting it when a task-ref selected a parent without one.
The batch-level `task_blocks` array carries the same parents in full; see
[Task blocks](#task-blocks).

Task-toggle results use kind `"task_toggle"`, `placement: "toggled"`,
`routed: true`, `text: ""`, and `task_line` set to the resulting task line.
They additionally include `toggle_direction` (`"link"` or `"unlink"` for the
`!` toggle, `"next"` for Ensure Next), `status_changed`,
`previous_task_line`, `status_symbol`, `status_name`,
`previous_status_symbol`, `previous_status_name`, `block_id`, `day_file`,
`block_link`, `removed_pomodoro_links`, `removed_scheduled` when a future
scheduled date was retired, and `schedule_log` when that retirement wrote an
entry under an existing Schedule Log. Link-direction toggles also report
`pomodoro_link_placement` when a link was inserted, `pomodoro_name` when a
named entry was selected or created, `creates_pomodoro`,
`pomodoro_already_linked`, and any later-link removals. Unlinking with `!`
reports the all-open-Pomodoro cleanup count and leaves the route note
untouched. `pomodoro_selector_unused` is always `false` and is not part of the
Ensure Next contract. Toggle results omit `sub_bullets`, `clip`, `priority`,
`priority_label`, `parent_*`, and `scheduled`. Ensure Next results add
`toggle_behavior`, `pomodoro_link_action`, and the
source/destination endpoint objects described under
[Task Link toggle](#task-link-toggle). An Ensure Next destination — and
any Pomodoro a link-direction toggle insert or unlink cleanup touches — also
appears in the batch-level `pomodoro_blocks` array;
see [Pomodoro blocks](#pomodoro-blocks).

When the link guard consumes a link or Ensure Next gesture, `link_guard`
describes why the target stayed unlinked and which prerequisites rose. For
`@route+block-id!`, the result omits `toggle_direction`, `block_link`,
`creates_pomodoro`, `pomodoro_already_linked`, `pomodoro_selector_unused`,
`pomodoro_link_placement`, `pomodoro_name`, `pomodoro_link_action`, and the
source/destination endpoints. Ensure Next keeps
`toggle_behavior: "ensure_next"` and omits the same insertion-only fields,
including `pomodoro_link_action` and its source/destination endpoints. For
both forms, `removed_pomodoro_links` and `day_file` report any cleanup at the
top level and are absent from the nested `link_guard` object.

Link-then-close is different: its Task Link lands under the Pomodoro being
closed as history, so the result retains both `block_link` and
`pomodoro_link_action` even when the task-side link guard reports a blocked
target. The standalone `@route:id` guard uses kind `"link_guard"`, so older
clients render it neutrally instead of claiming a link succeeded. Whenever
consumption removes live links, the top-level `removed_pomodoro_links`,
`day_file`, and batch-level `pomodoro_blocks` diff report that cleanup.

Task-complete results use kind `"task_complete"` with `placement:
"completed"`, `routed: true`, `route: null`, and `text: ""`.
`relative_target`/`target`/`route_label` name the task's note,
`task_line`/`previous_task_line` carry the post- and pre-images,
`block_id`, `previous_status_symbol`/`previous_status_name`,
`status_symbol`/`status_name`, and `status_changed` describe the root
transition, and the additive `task_complete` object reports `raw`, `note`,
`note_path`, `block_id`, `action` (`"completed"` or `"already_done"`),
`completion_date` (omitted when already done), `text` (the root task's
clean display text with the configured global filter, inline fields, and
trailing block ID removed; the same string the human output prints),
`subtasks` and `subtasks_left_open` (each with `note_path`, `block_id`,
`line`, `text`, status fields, and `reason` on left-open rows), `ledger`
(omitted when the daily note was untouched: `day_file`, `struck` count,
`struck_in` with one `line`/`name`/`status` entry per entry struck in
place, `moved` with `from`/`to` entry `line`/`name`/`status`,
`deduplicated` count, `dropped` with one `from`/`to` entry pair per
deduplicated bullet, and `removed_placeholders`), the extended
`unblocked[]` (each row adds `inbox`, `unblocked_by` with one
`note_path`/`block_id`/`text` entry per predecessor closed by this item,
`link` with `day_file` (vault-relative), `entry_name` (`""` when
unnamed), 1-based `entry_line` and `line`, `entry_created`, `next_up`,
`block_link`, and `block_id_created` — non-null exactly when this item
linked a picked offer — additive `offer` with `key`, `n`, `group`
(`hand_off` / `project` / `project_finished`), `project` on the project
group, `destination` `{day_file, entry_name, entry_role, fallback}`
(`entry_role` is `slot` / `closing` / `implicit` / `created`),
`selected`, and `block_id_needed`, `not_linked` with one of
`already_planned`, `project_task`, `project_member`, `hidden`,
`disabled`, `not_selected`, or `no_destination` (the key is omitted
when linked; `breaker` and `not_planned_today` are never emitted
again), `trigger` `"dependency"` or `"project_empty"` (a missing key
means an older bob), and `hide_removed` true when this item
removed a whole-token `#hide`), `still_blocked[]` (one `note_path`/
`block_id`/`line`/`text`/`status_symbol` row per open dependent that
stays blocked, including muted project-offer residents, with `reason`
`waits_on`, `scheduled`, or `project` (`project` carries the note's
vault-relative `project` path, plus `scheduled` when the project date
blocks), `waits_on` counting the open prerequisites left, `scheduled`
set only for `scheduled`, and the same `trigger` field), and
`unblocked_check` (`"checked"` when the lookup ran or the `[id::]` gate
proved it unnecessary, `"unavailable"` when the dependents snapshot
could not be built). Offered rows are emitted even when the status did
not change. `block_id` holds the minted ID when one was minted
(SL10), and `""` when the dependent has none and none was minted
(unpicked rows never mint).
`struck_in` and `dropped` are always present (possibly empty)
whenever `ledger` is present; `struck` and `deduplicated` keep their count
meaning. Entry `status` is `running`, `queued`, or `completed`; when an
entry has no name, `name` is `""`. The item's top-level `day_file` is the
absolute day file path whenever the item changed the day file (`ledger`
present or a successor linked); previously it was always `null`. After
all items are planned, net batch reporting drops any offer whose later
item linked, completed, re-blocked, or dropped it, or whose task is no
longer open (SL20). Every successful capture JSON (dry-run too) carries
top-level `pull_in` with `supported: true`, `offer_count`, `choice`
(`null` / `"all"` / `"none"` / `"subset"`), `picked_count`, and per
project-group `folded` counts. Dry-run JSON equals real-run JSON with
the same flags. The batch-level
`task_blocks` array gains roles `"completed"` (the root and each closed
subtask) and `"unblocked"` (each recovered or picked dependent); see
[Task blocks](#task-blocks). Human output prints
`✓ completed [*] → [x] <text>  <note> ^<id>` (dry-run: `would complete`),
one indented row per closed subtask and left-open descendant, a `ledger`
line in struck, moved, dropped, removed order ending with the day file
(an unnamed entry prints as `line N`), numbered `offer N` rows under
group headers, one `linked` row per picked successor
(`linked [?] → [*] <text>  <note> ^<id> → <destination>`,
dry-run: `would link`, with destinations `→ FIX`, `→ FIX (next up)`,
`→ new BOB session (next up)`, `→ new session`, and `→ line 37`, plus
`· added ^<id>` when an ID was minted), one `unblocked` row per
recovered-but-unlinked dependent (with suffixes `· already planned`,
`· not pulled in`, `· no open Pomodoro`, `· project task`,
`· project member`, `· hidden`, `· linking off`), one `surfaced` row per
project pull-in that removed `#hide` without linking or changing status
(dry-run: `would surface`; every project row puts `· project has no
open tasks` before any reason suffix; see
[`docs/task-dependencies.md`](task-dependencies.md) §12.10), and one
`still blocked` row per remaining blocked dependent (`· waits on N more`,
`· until YYYY-MM-DD`, `· project <path>`). A dry-run hint shows
`-p 1,2 · -p all`; a real run without a pick says the offers were not
pulled in. A left-open descendant or unblocked dependent with no
block ID prints only its note path with no trailing ` ^`. Already-done
tasks print `✓ already done [x] <text>  <note> ^<id> · nothing to change`.

Pomodoro-note results use kind `"pomodoro_note"` with `routed: false`, `route:
null`, and `target`/`relative_target` set to the daily note. They additionally
include `day_file`, `parent_line`, and `parent_text` describing the selected
Pomodoro's ledger line and text, but omit `block_id`, `block_link`,
`pomodoro_link_placement`, `parent_status_symbol`, and `parent_status_name`,
since the ledger checkbox is not an Obsidian task. Human output prints an
`under <parent_text>` line without a status marker, then the rendered
`- <text>` bullet. The noted Pomodoro also appears in the batch-level
`pomodoro_blocks` array; see [Pomodoro blocks](#pomodoro-blocks).

#### Pomodoro blocks

`bob capture -f json` (dry run and real run alike) gains one additive,
batch-level top-level key. It sits next to `plan_budget`, never appears per
item or inside `captures[]`, and is omitted when empty:

```json
"pomodoro_blocks": [
  {
    "relative_target": "2026/20260930.md",
    "line": 5,
    "name": "CLEANUP",
    "time_range": "0620-0735",
    "status": "running",
    "created": false,
    "roles": ["adjusted"],
    "lines": [
      {"text": "- [ ] (**0620-0735** [t:: 75m]) — CLEANUP", "depth": 0,
       "change": "changed", "before": "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP"},
      {"text": "\t- [[sase#^re-launch-failed]]", "depth": 1, "change": "unchanged"}
    ]
  }
]
```

Field rules:

- `relative_target` is the day file relative to the vault, `line` is the
  1-based headline line in the final staged day file, and `name` /
  `time_range` (plain `HHMM-HHMM`) come from the final ledger scan. Each is
  omitted when the entry has none. When `removed` is true, `line` is the
  1-based headline in the item's pre-state day text, so an earlier edit
  in the same item (N linked above T's consumption) does not shift it.
- `status` is `"completed"` for a completed entry, `"running"` for an open
  entry with a time range, and `"queued"` for an open entry without one.
- `created` is true when the entry did not exist before the batch.
- `removed` is true when this capture deleted the entry; omitted when
  false. A deleted entry's `lines` all carry `change: "removed"`.
- `roles` is an informational, deduplicated list in first-touch order. The
  vocabulary is `"adjusted"`, `"shifted"`, `"started"`, `"closed"`,
  `"reset"` (a demoted swap session or `x0` reset), `"next"`,
  `"linked"`, `"unlinked"`, `"removed"` (the entry this capture emptied),
  and `"changed"` (auto-detected);
  clients must not depend on it.
- `lines` is the block in document order — the headline plus every
  following line up to the first non-blank zero-indent line or the end of
  the `## Pomodoros` section, trailing blanks trimmed — with removed lines
  interleaved where they used to be. `text` is the verbatim line with no
  terminator; `depth` is the nesting level relative to the headline (the
  headline is 0, its direct children are 1, a non-list continuation line
  takes its parent list item's depth + 1, a blank line is 0); `change` is
  `"unchanged"`, `"added"`, `"removed"`, or `"changed"`; and `before` holds
  the old text only on `"changed"` rows. A created block's lines are all
  `"added"`. A deleted block's lines are all `"removed"`.

Blocks appear in first-touch order across the batch. A chain such as
`+2 x` touches the same session twice but reports it once, in its final
state, with the cumulative diff against the ledger before the capture.
Whole-item adjust, shift, start (including named starts), and close report
their sessions; link and task forms report theirs through auto-detection
until they carry explicit refs. Every `PomodoroBlockRef::removed`
producer reports the deleted entry this way, not just consumption.
Dry-run JSON equals real-run JSON except
for `dry_run`. Human output names a deleted session on the structured
`−` consumption lines
([Blocking existing tasks with '*'](#blocking-existing-tasks-with)).

#### Task blocks

`bob capture -f json` (dry run and real run alike) gains one additive,
batch-level top-level key next to `pomodoro_blocks`. It never appears per
item or inside `captures[]`, and it is omitted when empty:

```json
"task_blocks": [
  {
    "relative_target": "sase.md",
    "route": "sase",
    "line": 1,
    "block_id": "capture",
    "text": "Port capture to PIW sase-core",
    "status_symbol": "/",
    "status_name": "In Progress",
    "created": false,
    "roles": ["sub_bullet"],
    "lines": [
      {"text": "- [/] #task Port capture to PIW sase-core [created:: 2026-09-30] ^capture", "depth": 0, "change": "unchanged"},
      {"text": "\t- REQUIREMENTS", "depth": 1, "change": "unchanged"},
      {"text": "\t\t- existing", "depth": 2, "change": "unchanged"},
      {"text": "\t- Should reuse as much of PIW sase-core code as possible!", "depth": 1, "change": "added"},
      {"text": "\t- 🗓️ **SCHEDULE LOG**", "depth": 1, "change": "unchanged"},
      {"text": "\t\t- 2026-10-01 moved", "depth": 2, "change": "unchanged"}
    ]
  }
]
```

Field rules:

- One entry per distinct parent task the batch touched — sub-bullet
  parents, dependency writers, and whole-item `!` completions (roots,
  closed subtasks, and recovered dependents) — in first-touch order, in
  its final state.
- `relative_target` and `route` name the note. `line` is the 1-based task
  line in the final staged note. `block_id` is the parent's trailing block
  ID, omitted when it has none (a picker task-ref parent).
- `text`, `status_symbol`, and `status_name` describe the parent in the
  final state, with the same meanings as the per-item `parent_text`,
  `parent_status_symbol`, and `parent_status_name`.
- `created` is true only when the parent task did not exist before the
  batch. This happens when an earlier item in the same draft created it,
  as in `Task @sase^new` followed by `note @sase+new`. A created block's
  lines are all `added`.
  - `roles` is informational and deduplicated: `"sub_bullet"` for captured
  children, `"dependency"` / `"dependency_target"` for prerequisite writes,
  `"dependency_dropped"` when a capture link drops the task's own prerequisites,
  and `"completed"` (the completed root and each closed subtask) /
  `"unblocked"` (each recovered dependent) for whole-item `!` completions.
  Clients must not depend on it. A `dependency_target` block's `lines` include
  the staged BLOCKS child when that mirror changes; the item also carries
  additive `blocks_line_updates` and `minted_dependent_block_id` when a
  mint happens ([task-dependencies.md §14.11](task-dependencies.md#1411-outputs-and-warning-kinds)).
- `lines` uses exactly the `pomodoro_blocks` line schema: `text`
  (verbatim, no terminator), `depth` (relative to the task line, computed
  by the same helper), `change` (`unchanged` / `added` / `removed` /
  `changed`), and `before` (old text on `changed` rows only). Removed
  lines are interleaved where they used to be.
- The diff is cumulative against the note before the capture. If the same
  draft also rewrites the parent's task line (for example, a task toggle
  of the same task), the task line shows as `changed` with its `before`
  text.
- Dry-run JSON equals real-run JSON except for `dry_run`. Human output
  does not change. Per-item `parent_*` fields stay as they are.

#### Open notes

`bob capture -f json` (dry run and real run alike) always includes a
batch-level `open_notes` array next to `task_blocks` / `pomodoro_blocks`.
It never appears per item or inside `captures[]`. An empty array means
nothing to open (for example a queued link). When the key is absent, the
`bob` is older than this field. Clients should pass the entries through
rather than re-derive paths, lines, or digests.

```json
"open_notes": [
  {"path": "/Users/bryan/bob/mac_inbox.md", "relative_path": "mac_inbox.md",
   "role": "note", "label": "inbox", "line": 42, "ref": "42:35a733bd"},
  {"path": "/Users/bryan/bob/2026/20261017.md", "relative_path": "2026/20261017.md",
   "role": "day", "label": "Today", "line": 18, "ref": "18:0c1d2e3f"}
]
```

Selection: process items in source order. For each item, open its
`target` (skipped when empty, as for a queued link). Then open the day
file if this batch changed it (a staged text file whose
`updated_target != original_target`; the day file is the one
`pomodoro::day_file_for` returns). Deduplicate by path; the first item
to touch a path decides
where that note lands. Incidental writes do not open: BLOCKS mirrors,
the closed tasks' notes on `x`, and prerequisite notes.

Landing: prefer batch-level `task_blocks` / `pomodoro_blocks` lines
(already in final coordinates) and the `^block_id` task line found with
the existing task-line helpers. Otherwise fall back to the last exact
full-line match of `task_line` in the file's final text (the helper
behind `locate_new_task_ref`). Never use per-item line numbers; a later
item in the same draft can shift them. "First touched Pomodoro block"
means the first batch-level `pomodoro_blocks` entry for that file whose
role is not `removed`, in first-touch order; its `line` is the headline.

| Item kind | Where its note lands | Where the day note lands |
| --- | --- | --- |
| `task` (new task, any route or `^id`) | The new task line | First touched Pomodoro block |
| `bullet` (`@route#Section`) | The new bullet line | — |
| `sub_bullet` | The first added child line | — |
| `task_toggle`, `link_guard`, `pomodoro_link`, `pomodoro_task` | The task line carrying `^block_id` | First touched Pomodoro block |
| `pomodoro_start`/`_adjust`/`_shift`/`_close`/`_reset` (target is the day file) | First touched Pomodoro block | (same entry) |
| `task_complete` (`!`) | The completed task line (`task_blocks` role `completed`) | First touched Pomodoro block |
| `task_dependency` (`&`, `*` owner) | The owner task line (`task_blocks` role `dependency`) | — |
| `project_note` | The `^prj` line of the new note | First touched Pomodoro block |
| `ref` already in the library or intake | No line: open the note or PDF | — |
| `ref` queued | Nothing opens (empty target) | — |

Field rules:

- `path` is always absolute. A relative `BOB_DAY_FILE` is joined onto
  the vault directory.
- `relative_path` is vault-relative when the file is under the vault,
  otherwise the absolute path.
- `role` is `"day"` when the path is the batch's day file, else
  `"note"`. Clients must tolerate unknown roles.
- `label` is `"Today"` for the day file. Otherwise the contributing
  item's `route_label` when non-empty, else `relative_path` without
  `.md`.
- `line` is optional. A 1-based line in the file's final text (staged
  text for touched files, on-disk text for untouched ones). Omitted
  when bob cannot land.
- `ref` is present exactly when `line` is: `<line>:<digest>`, with the
  digest computed from that final line exactly as `note_tasks`
  computes it.
- `block_id` is optional. Present when the landed line carries
  `^block_id`.
- Dry-run JSON equals real-run JSON except for `dry_run`. Human output
  does not change.

#### Dependency updates and consumed links

Per-item additive fields on `bob capture -f json` (schema version 1).
Omitted when empty. Dry-run JSON equals live JSON apart from `dry_run`.

- `dependency_updates[]`: every dependency update the item made. The
  item's own `&` update comes first (`role: "dependent"`), then one
  entry per `*` target in typed order (`role: "blocked_task"`). Present
  whenever at least one update exists, so `&`-only items carry it too.
  The singular `dependency_update` stays and equals
  `dependency_updates[0]`. For `N *T` that is therefore T's update
  (dependent T, prerequisites [N]), which older Mac builds already
  render truthfully. Each entry keeps today's `dependency_update`
  shape (`dependent_note`, `dependent_block_id`, `dependent_text`,
  `new_task`, `added`, `already_present`, `open_prerequisites`,
  `prerequisites[]`, `dependent_status`, `dependent_status_name`,
  `status_changed`) and gains:
  - `role`: `"dependent"` or `"blocked_task"`
  - `previous_status` and `previous_status_name`
  - `blocked_residents`, the count of `^prj` residents newly blocked
    (omitted when 0)
  Each `prerequisites[]` row keeps `note`, `block_id`, `link`,
  `status_symbol`, `status_name`, `text`, and `open`. `status_symbol`
  and `status_name` are the post-write values. Additive fields:
  - `already_present`: `true` when this prerequisite was already on
    the Depends-On line (omitted when false)
  - `previous_status` and `previous_status_name`: present only when
    this write changed the prerequisite's status
- `consumed_links[]`: one row per live Task Link this item consumed.
  Fields: `{day_file, line, link, note, block_id, text, action,
  pomodoro: {line, name?, time_range?, status, removed}}`.
  `action` is `"removed"`, `"token_removed"`, or `"notes_kept"`.
  `status` is `"running"` or `"queued"`. `line` values are 1-based in
  the item's pre-state day text. `&` consumption emits the same array.
- `minted_blocker_block_id {path, line, block_id}`: present only when
  N's ID was minted for `*`. Never reuse `minted_dependent_block_id`,
  which names the ID minted so an `&` BLOCKS mirror can point at N as
  a dependent.
- Batch `pomodoro_blocks` entries for deleted sessions carry
  `removed: true` and role `"removed"`; see
  [Pomodoro blocks](#pomodoro-blocks).
- The legacy `warnings[]` string
  `removed N live Pomodoro link(s) to newly Blocked task(s)` stays for
  older clients. Human output prints the structured `−` lines instead,
  so the same fact is never printed twice.

See [Blocking existing tasks with '*'](#blocking-existing-tasks-with)
for the human lines and a full JSON example.

### Interactive editor markers

Bob Mac Capture (Control-Shift-Command-I by default) also supports incomplete
interactive markers. Use `<task> @:` to choose an area or project and then enter
a block ID, `<task> @route:` to prompt only for the block ID, or
`<task> @:block-id` to prompt only for the destination. A complete
`<task> @route:block-id` request captures immediately. The panel validates
each supplied or prompted component, emits only the canonical colon marker,
and retains staged values when validation or capture fails. Existing `@`,
`@#`, and `@route#` picker flows are unchanged.

Supported terminal `%`, `%N`, `%header`, `s:<N>`, and `$<N>` markers may
appear on either side of these interactive `@...` tokens and survive the
target, section, block-ID, or task picker. For example, `<task> @sase# %`
opens the section picker for `sase.md`; the panel consumes only `@sase#`, and
`bob capture` still owns clipboard, schedule, and priority interpretation
after the section is chosen.

Sub-bullet capture has the matching four-way `+` family. Use `<text> @+` to
choose a destination and then one of its open tasks, `<text> @route+` to choose
only the task, or `<text> @+block-id` to choose only the destination. A complete
`<text> @route+block-id` request captures immediately. The task chooser shows
each task's literal checkbox with status color and searchable status, block ID,
section, and child-note details; picker selections use stale-safe task refs.

Editor clients that speak the versioned JSON interfaces also support the
ordinary task-with-ID `^` family. Use `<task> @^` to choose a destination and
then author a new block ID, `<task> @route^` to prompt only for the new block
ID, or `<task> @^block-id` to prompt only for the destination. A complete
`<task> @route^block-id` request captures immediately as an ordinary task.
The right-hand block ID is user-authored and must be new, so completion is
deliberately route-only and never offers existing task block IDs for that side.
Appending the `+` sigil carries the project-note intent through the same
pickers: `<task> @^block-id+` still prompts only for the destination, while
`@^<id>+` reports the project-note mode with `needs: ["route"]` and a bare
`<task> @route^block-id+` is complete. The retired `<task> @:block-id+`
spelling offers nothing. Once the project note is addressed, typing ` :` or
` ^` at the end of a first-level task bullet opens the New ID flow: the
panel offers a `project_task_block_id` completion with the project-note stem
as its scope, body-derived suggestions, and the sibling IDs (plus `prj`) as
its used list. Accepting a row inserts the ID after the typed sigil — `:`
names the task Next and links it into the Pomodoro, `^` names it only — with
no teaching line.

A solo leading `^` opens the active-task picker instead of creating a task.
Typing `^` lists In Progress and Next tasks with block IDs, ordered by
today's open-Pomodoro Task Links (queued tasks in ledger order, then
unqueued In Progress, then unqueued Next); accepting a row inserts the full
`route:block-id` in one step. A typed `#name`/`=<X>`/`=x` suffix survives the
accept. `^`, `^fragment`, and `^route:` report `incomplete` with
`needs: ["active_task"]`, `^route:block-id#` needs `pomodoro_name`, and a
complete `^route:block-id[#pomodoro][=<X>]` reports `pomodoro_link` and links
the existing task on capture. A solo leading `:` opens the task-link picker
instead of creating a task: any single-token, single-line item starting with
`:` — `:`, `:dee`, even `:)` — reports `incomplete` with
`needs: ["task_link"]`, an empty body, `route`/`section`/`block_id` all
`null`, no diagnostics, and one `interactive_placeholder` span over the whole
token, sigil included. Multi-token (`: dee`), multi-line, and non-leading
(`Buy :dee`) shapes stay prose. A whole-item `x` close keeps the `pomodoro_close` span
out of completion gating, and typing `x` with nothing running surfaces the
`no running Pomodoro` diagnostic both in the CLI and in the Mac preview.

Bare-plus parent-task selectors report `incomplete` with
`needs: ["task_parent"]` and an `interactive_placeholder` over the full
`+query` token. This applies to a leading `+query`, prose-terminal `+query`,
and an eligible authored child line such as `- note +`. The exact lone `+`
stays `pomodoro_adjust` so the established five-minute action remains valid.
The editor completion endpoint still offers its dual-use parent picker for
that lone token. `bob capture` refuses unresolved plus selectors with a task
selection error; an accepted `@route+block-id` marker uses the existing
append/Ensure Next capture behavior.

## `bob capture-parse`

```bash
bob capture-parse [-f|--format human|json] [-R|--no-ref] [--] [TEXT]...
```

Reports the authoritative capture grammar's reading of `TEXT` so an editor can
highlight capture syntax and Obsidian wikilinks while the user is still typing.
It shares the tokenizer, terminal-marker extraction, and `@token`
classification with `bob capture`. Parse does not validate vault targets
or execute the draft, so a successful parse does not guarantee a capture
will succeed. Wikilink highlighting is syntax-only and additive; it does
not change capture routing or diagnostics.

The command is read-only and does not read the clipboard or contact the
network; it takes no `--bob-dir`. Unless `--no-ref` is set, it reads the URL
routing policy from the config file (`BOB_CONFIG_FILE`, otherwise
`$XDG_CONFIG_HOME/bob/config.yml`, otherwise `~/.config/bob/config.yml`) so a
bare link's `ref`/`task` classification matches capture. `-R, --no-ref`,
`highlights.url_routing.capture: false`, and an invalid config all keep
links as tasks. An invalid config is silent here. `bob capture`,
`bob gkeep list`, and `bob gkeep pull` print `URL routing is off: …` for
the same invalid file; `capture: false` and `gkeep: false` stay silent on
those commands too. A missing config file uses the defaults. Running
`capture-parse` with a Pomodoro-name span best-effort reads the configured
vault's theme catalog and adds a `themes` array to that span. If the catalog
is missing or unavailable, parsing still succeeds and omits those refs. The
catalog read never writes to the vault. A nonexistent `BOB_DIR` and a `%...`
clipboard marker still succeed. If `TEXT` is omitted and stdin is piped, it reads the
complete piped stdin stream, like `bob capture`. Only a missing `TEXT` or a
bad flag is an error (exit 2); every other input succeeds.

### Template drafts

See [Bulk templates](#bulk-templates) for the `@@template:` grammar. For
`capture-parse`, the header line is a `template_header` span, each `{N}` is a
`template_placeholder` span with its zero-based `field`, and every lexed row
value is a `template_field` span with its field index. Field spans include
quotes when a value is quoted. The JSON `template` object reports the header,
template block range and line bounds, `field_count`, and `rows`. Each row has
its one-based row number, source line and range, fields, and (when expansion
succeeded) the expanded capture `mode` and `body`. Rows of a one-placeholder
template can share a `line`, and each row's `range` is its value's range.

`items[0]` is the template parsed through a same-length masked copy. Each
expanded row follows as an item with mode `template_row`, the row's typed
range and source line, its expanded body and capture markers, and
`needs: []`. Its `template_row` value mirrors the corresponding entry in
`template.rows`. A row that cannot expand still has an item and its field
spans, but no expanded `mode` or `body`. If the header has no template yet or
the template has no rows, the top-level `needs` includes `template_row`.

Template-language diagnostics include `template_header_misplaced`,
`template_missing_rows`, `template_needs_separator`,
`template_missing_placeholder`, `template_invalid_placeholder`,
`template_placeholder_gap`, `template_row_bullet`,
`template_unterminated_quote`, `template_row_arity`,
`template_global_declaration`, `template_too_large`,
`template_clipboard`, and `template_session_operator`. A validly expanded row
that still has picker needs or parses as `incomplete` adds
`template_row_incomplete`. Without a header, a first item containing `{N}`
followed by another item adds the `template_header_missing` warning; capture
continues to treat those items normally.

Template and masked-parse ranges point into the typed draft because masking
preserves byte offsets. Per-row diagnostics use the expansion source map:
literal-template errors point to the template, an error touching one row field
points to that whole field (including its quotes), and an error touching
multiple fields points to the row. Repeated literal errors are deduplicated by
code, range, and message. A placeholder that changes a token's meaning per
row, such as `${0}`, is highlighted according to how the masked template
parses it; each row's expanded parse still reports any real problem.
`global_destination` is omitted for template drafts. Drafts without a header
keep their existing JSON shape, except for the `template_header_missing`
warning when that hint applies.

`TEXT` accepts the same batch draft `bob capture` does: one or more blank or
whitespace-only physical lines separate capture items, and each item keeps the
existing parent-plus-authored-bullets grammar. Within an item, the first
physical line is the parent, later column-zero `-`/`*`/`+` lines become
first-level authored children, and later lines prefixed by exactly two ASCII
spaces become nested authored children. Separator rows themselves have no
marker completion or highlighting. Incomplete
interactive markers are valid input rather than errors, so `@`, `@#`,
`@#Ideas`, `@route#`, `@^`, `@route^`, `@^id+`, `@+`, `@route+`, `@route+id#`,
`@:`, `@route:`, `@route:id#`, `@route:#name`,
and the legacy `@!` aliases all parse on any line. A valid `=<X>` start
suffix on a `@<route>:<block-id>[#<name>]` marker (including the short `=name` suffix, e.g. `@work:outline=bugs`) parses as `pomodoro_task`
with a `pomodoro_start` object (`raw` plus 5-minute `duration_units` and
`offset_units`) and a `pomodoro_start` span covering the `=` and `<X>`
bytes; the Pomodoro-name span always ends before the `=`, even on incomplete
markers such as `@<route>:=3`. On a link item the suffix name emits `pomodoro_start [13,14)` plus `pomodoro_name [14,18)` spans (e.g. `@work:outline=bugs`). An invalid suffix is an
`invalid_pomodoro_start` diagnostic, including the name-once error (`@work:outline#bugs=focus`) and the link drop-list error (`@work:outline=bugs~2`: a link start takes a timing and a name, not a drop list). A whole-item `+[N]`/`-[N]` adjustment
(for example `+5` extends by 25 minutes, `-` shortens by 5 minutes) parses
as `pomodoro_adjust` with a `pomodoro_adjust` object (`raw` plus sign and
5-minute `units`, defaulting to 1 when the count is omitted) and a
`pomodoro_adjust` span covering only the signed token; the parse stays
purely lexical and never guesses current ledger times. A whole-item
`++[N]`/`--[N]` shift (for example `++3` moves 15 minutes later, `--` moves
5 minutes earlier) parses as `pomodoro_shift` with a `pomodoro_shift`
object (`raw`, direction as `later`, and 5-minute `units` defaulting to 1)
and a `pomodoro_shift` span covering only the operator token. A `@@`
declaration still routes ordinary items in the same draft but never turns
an operator into a task. Invalid standalone counts (`+0`, `++0`, overflow)
and malformed operator-first items (extra text, markers, or child lines)
report an `invalid_pomodoro_adjustment` (one sign) or
`invalid_pomodoro_shift` (two signs) diagnostic. A whole-item `=`/`=<X>`
start (for example `=` is 25 minutes, `=3` is 15 minutes) parses as
`pomodoro_start` with a `pomodoro_start` object (`raw` excludes `=`, plus
5-minute `duration_units`/`offset_units` and the additive `drop` list) and a
`pomodoro_start` span covering the whole token (only `=<X>` when a `~<K>`
drop part is present, with a `pomodoro_start_drop` span covering `~<K>`
including the `~`); the human `start` line reads `=3#bugs~1,3 (15m, offset
0u · drop 1, 3)`. A claimed token (counted or drop-carrying) with extra
text, an exact token with child lines, or an oversized suffix reports
`pomodoro_start` plus an `invalid_pomodoro_start` diagnostic (the extra
text, the child line, or the token for overflow). A trailing `~<K>` drop
list parses with the sorted list in the spec; a dangling `~`/`,` reports
mode `incomplete` needing `pomodoro_start_task` with the partial spec, the
spans typed so far, and one `interactive_placeholder` span over the
separator. A whole-item `=name` named start (for example
`=bugs` is 25 minutes, `=3#bugs` is 15 minutes, `=bugs~2` starts
`BUGS` without link 2) parses as
`pomodoro_start` with the same `pomodoro_start` object (`raw` excludes `=`,
so `=3#bugs` reports `"3"`), `section` carrying the typed name, a
`pomodoro_start` span covering the `=` plus `<X>` bytes (`[0,1)` on `=bugs`), and a `pomodoro_name` span
covering the name (`[1,5)` on `=bugs`). A bare token emits `pomodoro_start [0,1)` and `pomodoro_name [1,1+len)`, then the existing `@<X>`/`~<K>` spans; modes, needs, specs, and diagnostic codes are identical to the `#` spelling. `=<X>#` with an empty name is an editing state, never a
mistake: mode `incomplete` needing `pomodoro_name`, with the partial spec,
the `pomodoro_start` span typed so far, and one `interactive_placeholder`
span over the `#`. Every other named-start near miss reports
`pomodoro_start` plus an `invalid_pomodoro_start` diagnostic reusing
`bob capture`'s exact wording: an empty name with extra text or child
lines, a link-form order (`=#bugs=3`, ranged on the name), invalid name
characters (ranged on the name), an oversized suffix (ranged on `=<X>`), or
extra text, markers, or child lines on a well-formed token (ranged on the
extra text or the child line, with the join-with-`-` hint when the extra
text is all name characters; diagnostics that rewrite a token keep that token's spelling, e.g. `=deep work` → `=deep-work` vs `=#deep work` → `=#deep-work`). An exact-name literal (`=#"DEEP WORK"`) parses the same way with `section` carrying the decoded name: the `pomodoro_name` span covers the whole literal including its quotes (`[2,13)` on `=#"DEEP WORK"`), and the same holds for literal link suffixes (`[14,24)` on `Do work @r:id#"SASE V18"`). The glued-timing teaching error (`=3bugs`, `=2fa`) fires when the byte after a non-empty `<X>` is an ASCII letter, suggesting `=bugs@3` or `=3#bugs`. A `@@` declaration never applies to named
starts either. A claimed `==` start form is retired: `capture-parse` reports
`invalid_pomodoro_start` with the same teaching message as `bob capture`,
including a rewrite that removes one leading `=`. For example, `==` reports
`` `==` is retired: `=` restarts or swaps the running Pomodoro by itself; write `=` ``.
A bare `==` followed by prose stays prose, protecting Obsidian highlights
(`==important== thing`, `==foo`, `== foo`, `===`, and mid-body `Plan ==3`).
The claimed close near misses `==x`, `==X1`, `==*`, and `==!2` report that
they are not closes and teach the corresponding lowercase `x` form. In a
chain such as `x ==`, the `==` token fails on its own range with the retired-
form diagnostic. Parse output does not carry an override flag because
restarting or swapping depends on the staged daily ledger, which only
`bob capture` reads. A whole-item `x[<N>][*<P>][!<M>][~<K>]` close
(lowercase `x` only, with `*`, `!`, and `~` in any order) parses as
`pomodoro_close` with a `pomodoro_close` object (`raw` plus the additive
`in_progress` list, `null` when no `<N>` was typed, the `park` list,
the `complete` list, the `drop` list, additive `park_all` and `complete_all` booleans, and the
`log` entries (`index`, `text`, `details`) in typed order; wildcard groups keep
their lexical lists empty while the close summary resolves them to concrete
ascending task indices; `index` is omitted for unnumbered bullets under a close
without `<N>`/`*<P>` or with wildcard intent, which `bob capture` resolves
after staging the running session lineup) and
spans covering the one-byte `x` token (`pomodoro_close`), the `<N>`
list including its commas (`pomodoro_close_in_progress`), the `*<P>` list
including the `*` (`pomodoro_close_park`), the `!<M>` list
including the `!` (`pomodoro_close_complete`), the `~<K>` list including the
`~` (`pomodoro_close_drop`), and each numbered entry index
(`pomodoro_close_log_index`; positional entries get no index span, and entry
and detail text render as neutral prose
but keep wikilink spans); the human `close` line reads
``x1,3!2~4 (in progress 1, 3 · complete 2 · drop 4 · defer the rest)``, with
`in progress none` for `x0`, a bare `x` for a plain close, and
`log 2 "wired the lexer" (+1 detail)` for typed entries with details
(`log "wired the lexer"` with no number for entries resolved at execution).
One entry may sit on the close line (`x wired it` logs to task 1 with `body`
`x`, or to the first eligible top-level worked link under wildcard intent; an
explicit number gets a `pomodoro_close_log_index` span while a runtime-resolved
default gets none); a dangling inline number (`x 2`) reports mode
`incomplete` needing `pomodoro_close_log_text` with the partial spec and a
placeholder over the number. Several entries use child bullets below the
close (`- [<n>] <text>`: bullets are numbered all or none, and unnumbered
bullets log in order to the close's worked tasks); a dangling bullet (`- 1`)
reports mode `incomplete` needing `pomodoro_close_log_text` with the partial
spec (lists plus every complete entry with its details) and a placeholder
over each dangling number instead of its index span, while a bad bullet (mixed
numbering, too many unnumbered bullets, a close that works no task,
a non-loggable index, a bad number, a block link, a fence) reports
`pomodoro_close` plus an `invalid_pomodoro_close` diagnostic on the precise
range, as does a bad inline entry. Every malformed list (a
duplicate, an overlap, a misplaced `0`, a second `!` or `~`, a bad
character, an oversized number, or a space inside the lists, which gets the
no-spaces hint) reports the same. A token ending
in a dangling separator (`x1,`, `x!`, `x~`, `x1!`, `x!2,`, `x0!`)
reports mode
`incomplete` needing `pomodoro_close_task`, with the partial spec typed so
far, the spans typed so far, and one `interactive_placeholder` span over the
separator. A same-line chain splits into one `items[]` entry per operator,
with child lines attaching to the line's `x`; a chain whose `x` is not
last nests its item ranges, so they never partially overlap. Other
`=`-prefixed tokens (`= foo`, `= bugs`)
and mid-body `x`/`=3`/`=bugs` stay ordinary prose. A bare token never emits the `interactive_placeholder` that the empty `=#` uses; lone `=` stays a complete start. On link items the
`=x…` suffix spans the same four span kinds instead of `pomodoro_start`:
`@r:id=x1!2` and `^r:id=x1` stay `pomodoro_link` (or `pomodoro_task` with body
text) and carry the spec, `^r:id=x1,`, `^r:id=x1!`, and `^r:id=x1~` report
`incomplete` needing `pomodoro_close_task`, while `#name=x…`,
`s:<N>`/`$<N>`/`%` conflicts,
project-note `=x`, and malformed lists report `invalid_pomodoro_close` on
the conflicting component or the precise list range. A `@@` declaration
never applies to close or `=`/`=<X>` items, and neither is ever rewritten.
A start token or a close token requests no completion: a cursor on that
token, including anywhere inside the task-number lists or a dangling
separator, returns an empty success. In Work Log text under or on the
close line, marker and block-link completion are suppressed and note and heading
completion keep working. The picker's in-progress
`@^`, `@route^`, `@^id`, `@:`, `@route:`, and `@:id` spellings are unchanged,
and `@^id+` carries the project-note intent with `needs: ["route"]`. A `:` project-note
spelling (`@:id+`, `@route:id+`, `@route:id+#name`) is a
`retired_project_note_marker` diagnostic teaching the `^` form instead. The retired
`@route::...` spelling is a diagnostic directing users to `@route^...`;
it is not an incomplete Pomodoro marker. A trailing bare `#` is a complete
`pomodoro_note`, not an incomplete section marker. Complete and in-progress Obsidian links such as `[[sase`,
`![[sase]]`, `[[sase#Design|Spec]]`, and `[[#^block-id]]` also parse for
semantic highlighting. An invalid marker component, a malformed continuation
line, an orphaned nested bullet, an item emptied by marker removal, or a
duplicate item-wide marker across lines becomes a diagnostic instead of a
failure, while `bob capture` keeps its strict execution errors for the same
text.

JSON output is a single versioned object:

```json
{
  "ok": true,
  "schema_version": 1,
  "input": "Call bank @Cash+",
  "body": "Call bank",
  "mode": "incomplete",
  "route": "cash",
  "section": null,
  "block_id": null,
  "needs": ["task"],
  "spans": [
    { "start": 10, "end": 15, "kind": "sub_bullet_route" },
    { "start": 15, "end": 16, "kind": "interactive_placeholder" }
  ],
  "diagnostics": []
}
```

`input` is the raw text as received, before whitespace normalization. `body` is
the normalized capture body after terminal `s:<N>`, `$<N>`, and applicable
`%...` markers and the recognized `@...` token are removed, matching what `bob capture` would
write for any input it accepts. `mode` is `task`, `bullet`, `pomodoro_task`,
`pomodoro_note`, `sub_bullet`, `task_toggle`, `project_note`,
`pomodoro_project_note`, `pomodoro_adjust`, `pomodoro_shift`,
`pomodoro_link`, `pomodoro_close`, `pomodoro_start`, `task_dependency`, `task_complete`, `ref`, or `incomplete`, describing whichever line resolved a marker
first -- the parent's leading or trailing form, or else the first child line
with a trailing marker. A whole-item bare link reports mode `ref` with one
`ref_url` span covering the whole token, `<>` included. In an eligible URL
reference, exact standalone `URL %` (or compatible `URL @`) adds
`ref_listen: true` and a `ref_listen` span over the directive; `-R` keeps it a
`task` so parse and capture always agree. A solo `@route:block-id…` item reports
`pomodoro_link` with the `pomodoro_route` / `pomodoro_block_id` /
`pomodoro_name` / `pomodoro_start` spans (or `pomodoro_close` plus the two
list spans for an `=x…` suffix); the `^` spelling reports the same
mode with `active_task_route` (covering `^route`) and `active_task_block_id`
spans plus the existing name and start/close spans. A whole-item `x…`
reports `pomodoro_close` with its spans and spec (a dangling `,`/`!`
reports `incomplete` with the partial spec instead), and a
whole-item `=`/`=<X>` reports `pomodoro_start` with a `pomodoro_start` span
and spec, both with `needs: []` (the incomplete close needs
`["pomodoro_close_task"]`). A whole-item `=<X>#pomodoro` reports
`pomodoro_start` with the `pomodoro_start` / `pomodoro_name` spans, the
spec, and `section` set to the typed name, also with `needs: []`, while
`=<X>#` reports `incomplete` with `needs: ["pomodoro_name"]`. Lone `^`, `^fragment`, and
`^route:` report `incomplete` with `needs: ["active_task"]` and one
`interactive_placeholder` span over the token, while `^route:block-id#` needs
`pomodoro_name`. Near misses and solo-link conflicts report an
`invalid_pomodoro_link` diagnostic; the additive `pomodoro_start` spec is
unchanged. The `project_note` / `pomodoro_project_note` split
mirrors the `task` / `pomodoro_task` one, so a client can tell whether the
daily note is involved: a project note with at least one ` :` task is
`pomodoro_project_note`, and a `^`-only one stays `project_note`. A lone
`:` or `^` ending a first-level bullet is an unfinished ID: mode
`incomplete` with `needs: ["block_id"]` and an `interactive_placeholder`
span over the sigil, never a diagnostic. A bare trailing `#` reports `pomodoro_note` with
`route`, `section`, and `block_id` all `null` and an empty `needs` list. Combining
that marker with `@route`, `s:<N>`, or `$<N>` on the same item still reports
mode `pomodoro_note` plus a `pomodoro_note_conflict` diagnostic; `bob capture`
rejects the same input. A trailing `#n` or `#no` is rejected like any other
trailing `#tag`. `route`, `section`, and `block_id` are the
resolved components, or `null`; `block_id` carries the ID-only task, Pomodoro,
sub-bullet, or project-note ID, whichever applies — for a project-note marker
it is the authored block ID the filename suffix derives from. For a Pomodoro
marker, `section` carries
the Pomodoro name when one was typed — the same "whichever applies" reuse
`block_id` already has, and `mode` disambiguates — and the same holds for the
`^` project-note form, where `section` is the `#pomodoro` name. `needs` lists what a picker
still has to supply, in the
order `route`, `section`, `block_id`, `pomodoro_id`, `pomodoro_name`, `task`, `task_section`, `active_task`, `task_link`, `task_goto`, `task_parent`, `pomodoro_close_task`, `pomodoro_close_log_text`, `pomodoro_start_task`, `task_dependency`, `blocked_task`, `dependency_target`, `task_complete`; it is an independent
completion hint, so the executable `@route#` bullet reports mode `bullet` and
needs `["section"]`, while `@route+id#` with no body text reports mode
`incomplete` and needs `["pomodoro_name"]`, `note @route+id#` reports mode
`incomplete` and needs `["task_section"]`, and `@route:id#` reports mode
`incomplete` and needs `["pomodoro_name"]`. A complete `@route+id#sec`
sub-bullet populates `route`, `block_id`, and `section` together. A complete
`@route:id#name` Pomodoro marker and a complete `@route+id#name` task toggle
populate `route`, `block_id`, and `section` the same way; the mode says whether
`section` is a Pomodoro name or a task section.

`relationship_needs_text` is an error diagnostic, not a picker need. It is
ranged on the `$<N>`, `s:<N>`, clipboard marker, or `child bullets` token
when a bare existing-task owner has relationships but no task text; the
diagnostic message is identical to the `bob capture` refusal. See
[Relating existing tasks with `&` and `*`](#relating-existing-tasks).

`sub_bullets` is an optional array, omitted when empty, of every other valid
physical line's normalized body -- its source `-`/`*`/`+` marker and any
item-wide markers already removed -- in source order. These are semantic
parse bodies for an editor's own preview, not rendered Markdown: they carry
no target-selected indentation or `- ` marker, unlike `bob capture`'s own
`sub_bullets` output field. When `sub_bullets` is present,
`sub_bullet_depths` is an aligned optional array of `1` and `2` values, one
per body, so version-tolerant clients can preserve hierarchy without a
breaking schema change. Older clients may ignore the additive field; clients
talking to an older `bob` that omits it should treat every body as depth `1`.
`sub_bullets` bodies exclude the ID token: `- Draft the memo :draft-memo`
parses as body `Draft the memo`. A parallel `sub_bullet_task_ids` array,
emitted at the top level and per item only when at least one entry is
non-null, carries `null` or `{"block_id", "link"}` per body (`link` is true
for `:`). Human output shows each sub-bullet with its ID, as ` ^id` or
` :id`. This remains schema version 1.

For a multi-item draft, `items` is an ordered optional array, omitted for a
single item. A same-line session-operator chain reports one `items[]` entry
per token: each `range` covers its token, and all entries share the physical
line's `line_start`/`line_end`. Each entry has a one-based `index`, a `range` with global UTF-8
`start`/`end` offsets into `input`, `line_start`/`line_end` physical line
numbers, the item's `body`, `mode`, `route`, `section`, `block_id`, `needs`,
and optional `sub_bullets`/`sub_bullet_depths`/`sub_bullet_task_ids`/`pomodoro_start`/`pomodoro_adjust`/`pomodoro_shift`/`pomodoro_close`/`dependencies`/`blocked_tasks`/`dependency_target`/`task_complete`. Real item indices and ranges
exclude declaration-only `@@` lines but still index the original draft. Top-level and
per-item `route`, `mode`, and `block_id` are the item's effective destination
after inheritance. The legacy top-level fields continue to describe the first
item so older clients retain a useful preview. `blocked_tasks` is the `*`
twin of `dependencies[]`: the same `{raw, note, block_id, quoted, range}`
shape, emitted at the top level and per item, omitted when empty. Never put
a direction flag on `dependencies[]`. A partial `*` needs `blocked_task`;
see [Blocking existing tasks with '*'](#blocking-existing-tasks-with).

`relations[]` is the `~` / `~~` analogue: `{raw, direction: "to"|"from",
note, block_id, quoted, label, label_range, range}`, emitted at the top
level and per item, omitted when empty. Labels come from `|label` /
`|"label text"` suffixes and are validated against the
[relation label rules](task-relations.md#32-labels). A partial `~` /
`~~` needs `relation_target`; a label position needs `relation_label`.
An ownerless item reuses `dependency_target`, mode stays
`task_dependency`, and `relationship_needs_text` covers `~` / `~~`
exactly as it covers `&` / `*`. Span kinds `relation_sigil`,
`relation_note`, `relation_block_id`, `relation_label_separator`, and
`relation_label` mark each part. Top-level `relation_grammar: 1`
appears whenever the draft contains a `~` byte; drafts without `~`
are byte-identical to today, and relations never appear in
`dependencies[]` or `blocked_tasks[]`. See
[Linking tasks with '~' and '~~'](#linking-tasks).

When the draft has a `@@` declaration, `global_destination` is an optional
object with the declaration `range`, one-based physical `line`, effective
`mode`, `route`, `block_id`, and `needs`. It is omitted when no declaration is
present, so schema version 1 stays additive.

`pomodoro_start` is an optional object, omitted for every input without a
start suffix or whole-item start, with the typed `raw` `<X>` text (excluding
`=` on whole-item starts, so `=3` reports "3" and `=` reports "") plus
5-minute `duration_units` and `offset_units`, so schema version 1 is
unchanged for older inputs. It describes only the typed start; override
state is derived by `bob capture` from the staged ledger and appears in its
`pomodoro_start.override` result object. Multi-item drafts report each
item's own `pomodoro_start` alongside the top-level preview of the first
item; start items never inherit a `@@` declaration.

`pomodoro_adjust` is an optional object, omitted for every input without an
exact adjustment, with the typed signed `raw` token plus its sign (`plus`
true for `+[N]`, false for `-[N]`) and 5-minute `units` (so `+5` is 25
minutes and `-` is 1 unit), so schema version 1 is unchanged for older
inputs. Multi-item drafts report each item's own `pomodoro_adjust`
alongside the top-level preview of the first item; adjustment items never
inherit a `@@` declaration.

`pomodoro_shift` is an optional object, omitted for every input without an
exact shift, with the typed operator `raw` token plus its direction
(`later` true for `++[N]`, false for `--[N]`) and 5-minute `units` (so
`++3` is 15 minutes later and `--` is 1 unit earlier), so schema version 1
is unchanged for older inputs. Multi-item drafts report each item's own
`pomodoro_shift` alongside the top-level preview of the first item; shift
items never inherit a `@@` declaration.

`pomodoro_close` is an optional object, omitted for every input without a
close, with the typed `raw` close text exactly as typed (`x` or `x1,3!2~4`
for a whole-item close, `=x!1` for a link suffix), the `in_progress` list
(`null` when no `<N>` was typed, `[]` for `x0`, otherwise the typed
numbers sorted ascending), and the `complete` and `drop` lists. The `drop`
field is omitted when no `~<K>` list was typed, so older closes keep their
version 1 shape. In `x0~2`, `0` is the entire in-progress list; task 2 is
dropped. An incomplete close (`x1~`) reports the partial lists typed so far.
A lexically invalid close
token reports no `pomodoro_close` object at all, only the
`invalid_pomodoro_close` diagnostic. Multi-item drafts report each item's
own `pomodoro_close` alongside the top-level preview of the first item;
close items never inherit a `@@` declaration.

`pomodoro_reset` is an optional object, mutually exclusive with
`pomodoro_close`, for a note-free plain `x0` that returns
the current Pomodoro to the first future placeholder. The parser still
reports the syntactic close input (it cannot know the vault-dependent
outcome), so `capture-parse` keeps describing `x0` while `bob capture`
resolves it to `pomodoro_reset` or `pomodoro_close`. Whole-item resets report
`kind: "pomodoro_reset"` with `placement: "updated"`; attached link/new-task
forms keep their link/task kind and placement and add `pomodoro_reset`. The
object carries `raw`, `day_relative`, optional `pomodoro_name`,
`previous_pomodoro_line`, `pomodoro_line`, `previous_entry_line`,
`entry_line`, `previous_time_range`, `time_range: null`,
`created_pomodoro: false`, and `moved`. Batch-level `pomodoro_blocks` shows
the reset block with a `reset` role in its final open/untimed state. Schema
version stays 1; older clients ignore the additive key and fall back to
generic capture/block presentation.

`spans` are UTF-8 byte offsets into `input`, half-open `[start, end)`, ordered,
non-overlapping, and always on a character boundary. Each `kind` is one of
`route`, `section`, `task_block_id_route`, `task_block_id`,
`pomodoro_route`, `pomodoro_block_id`, `pomodoro_name`, `pomodoro_start`, `pomodoro_adjust`, `pomodoro_shift`, `pomodoro_close`, `pomodoro_close_in_progress`, `pomodoro_close_park`, `pomodoro_close_complete`, `pomodoro_close_drop`, `active_task_route`, `active_task_block_id`, `pomodoro_note`, `project_note_marker`, `sub_bullet_route`,
`sub_bullet_block_id`, `sub_bullet_section`, `task_toggle_route`,
`task_toggle_block_id`, `task_toggle_pomodoro_name`, `task_toggle_explicit_toggle`, `global_route`,
`global_sub_bullet_route`, `global_sub_bullet_block_id`, `schedule`, `priority`, `clipboard`,
`ref_url`, `ref_listen`,
`interactive_placeholder`, `project_task_link_marker`, `project_task_block_id`, `dependency_sigil`, `dependency_note`, `dependency_block_id`, `blocked_task_sigil`, `blocked_task_note`, `blocked_task_block_id`, `task_complete_sigil`, `task_complete_note`, `task_complete_block_id`, `wikilink_delimiter`, `wikilink_target`,
`wikilink_heading`, `wikilink_block_id`, or `wikilink_alias`. A placeholder
marks the part of a marker the user has not filled in yet: the trailing `+` in
`@cash+` or `@@cash+`, the trailing `#` in `@cash+id#` or `@cash:id#`, the dangling `,` or `!` in `x1,` or `x!`, a lone `:` or `^` ending a project-note task bullet, or the whole `@+` /
`@@` when the route is still empty too. For example, `x1,3!2` gives
`[0,2) pomodoro_close`, `[2,5) pomodoro_close_in_progress`, and
`[5,7) pomodoro_close_complete`; `x1,3!2~4` adds
`[7,9) pomodoro_close_drop`. Link forms put these spans after the route and
block-ID spans. A project-note marker adds one
`project_note_marker` span covering the single `+` byte; its route and
block-ID components keep their base-family span kinds (`task_block_id_route`
/ `task_block_id`), and its `#name` keeps the `pomodoro_name` span. An
accepted task ID on a project-note bullet adds a `project_task_link_marker`
span over the `:` sigil only, plus a `project_task_block_id` span over the ID
for both sigils; the `^` sigil gets no span, just as separators never do.
Wikilink spans cover syntax only; unresolved note targets are not errors.

Each entry in `diagnostics` has `severity` (`error`, `warning`, or `info`), a
stable snake_case `code`, a `message` reusing `bob capture`'s exact wording,
and a nullable `range` given as a two-element `[start, end]` byte array.
Today's codes are `invalid_task_block_id_route`, `invalid_task_block_id`,
`retired_task_block_id_marker`, `invalid_sub_bullet_route`,
`invalid_sub_bullet_block_id`, `invalid_sub_bullet_section`,
`invalid_pomodoro_route`, `invalid_pomodoro_block_id`, `invalid_pomodoro_name`, `invalid_pomodoro_start` (a malformed `=<X>` suffix, one on a project-note `+` form, a whole-item start near miss such as `=3 more` or `=3x`, or a whole-item named-start near miss such as `=#bugs=3`, `=#deep work`, or `=3#bugs more`, with the range on the extra text, child line, name, or token for overflow), `invalid_pomodoro_adjustment` (a zero magnitude, an overflow, or extra text/markers/child lines on an adjustment-first item), `invalid_pomodoro_shift` (a zero magnitude, an overflow, or extra text/markers/child lines on a shift-first item), `invalid_pomodoro_close` (a leading selection-shaped token with extra text/markers/child lines, a malformed task-number list, `#name=x…`, a project-note `=x`, or an `s:<N>`/`$<N>`/`%` conflict on a close item, with the range on the extra text, child line, offending list part, `#name`, or conflicting marker), `invalid_pomodoro_link` (a near miss or conflict on a solo `@route:block-id…` / `^route:block-id…` item, such as extra text, authored children, or terminal markers on a complete shape), `invalid_project_note_marker` (a project-note shape error: a misordered `+`, a `#name` without its `+`, or an `=` suffix), `retired_project_note_marker` (a `:` project-note spelling, teaching the `^` form), `misplaced_project_task_id` (a task ID on the parent line or a nested bullet), `invalid_project_task_id` (a malformed, reserved, empty-body, or checkbox-bearing ` :` / ` ^` task ID), `duplicate_project_task_id` (two equal IDs in one item, naming both lines), `unused_project_note_pomodoro` (a `#name` with no ` :` task, ranged on the name), `unsupported_explicit_toggle`, `legacy_bullet_marker`, `retired_priority_marker` (a typed `p:<N>` token, teaching `$<N>`),
`pomodoro_note_conflict` (a trailing bare `#` on the same item as `@route`,
`s:<N>`, or `$<N>`),
`invalid_child_line` (a later physical line is not blank, a column-zero
authored bullet, or a two-space nested authored bullet),
`orphaned_nested_bullet` (a nonempty nested item has no preceding first-level
authored owner), `empty_child_after_markers` (an authored bullet has no text
left once its capture markers are removed),
`duplicate_capture_marker` (a later line in the same item resolves a route,
schedule, priority, or clipboard marker a prior line already resolved),
`invalid_global_destination` (unsupported or malformed `@@` declaration),
`duplicate_global_destination` (a later `@@` declaration after the first),
`global_destination_shadowed` (warning: an item-local marker overrides the
declaration token on that same item), and `missing_capture_item` (a declaration
with no capture item). Human output
prints the same information without color escapes when piped — including a
`start` line such as `=3 (15m, offset 0u)` when a suffix or whole-item start is present (whole-item `=` reports `= (25m, offset 0u)`), an `adjust` line such as `+5 (25m, 5 units)` (or `- (5m, 1 unit)`) when an adjustment is present, a `shift` line such as `++3 (15m later, 3 units)` (or `-- (5m earlier, 1 unit)`) when a shift is present, and a `close` line such as `x1,3!2 (in progress 1, 3 · complete 2 · defer the rest)` when a close is present (a plain close prints a bare `x`) — plus a
`Sub-bullets` section listing `sub_bullets` with indentation from
`sub_bullet_depths` when it is nonempty. On a missing `TEXT`, JSON mode prints
a single `{"ok": false, "error": "..."}` object on stdout and keeps stderr
clean.

## `bob capture-related`

```bash
bob capture-related [-b|--bob-dir DIR] [-d|--days N] [-e|--exclude NOTE:LINE] [-f|--format human|json] [-l|--limit N] [-q|--query TEXT] [-- DRAFT...]
```

This read-only command shows existing tasks that a capture draft may duplicate
or relate to. Draft mode follows the capture editor grammar and examines only
items that create tasks: `task`, `pomodoro_task`, `project_note`, and
`pomodoro_project_note`. For a project-note item it matches the title. A
`@@template:` draft and a draft with no task-creating items return an empty
`items` array before Bob scans the vault.

Open candidates are tasks in routable inbox, area, and active project notes.
Closed candidates are tasks completed or canceled in the last `--days` days,
including recent `done/` archives. Checklist rows, recurring tasks in closed
state, and daily notes are excluded. A task already named by a draft
dependency, blocked-task marker, relation, task destination, or parent-task
destination is listed under `linked` and omitted from the matches.

Each item is tokenized with Bob's task-title rules and ranked against the
candidate set. The bands are `same_title`, `possible_duplicate`, `similar`,
`closed_duplicate`, and `hold`; same-title matching is case-insensitive and
ignores punctuation, tags, fields, and emoji. Titles that name different
numbers, such as `Part 1` and `Part 2` or `2025` and `2026`, can be
`similar` but are never `possible_duplicate` or `closed_duplicate`. The
exact score bars and default action mapping can change without a
schema-version bump. Draft JSON never includes scores. The separate
`--query TEXT` harness ranks one title and includes numeric scores for
calibration. With `--query`, `-e|--exclude NOTE:LINE` removes exactly one
candidate before scoring and ranking, for example `--exclude dev.md:42`. The
note is vault-relative and the line is 1-based. This allows leave-one-out
evaluation to see recently closed matches that the query task itself would
otherwise suppress. It is invalid in draft mode; no file is edited.

**Draft JSON contract.** Success uses `schema_version: 1` and returns
`ok`, `warnings`, and `items`. Each item contains:

- `index`: its 0-based position in the full capture draft;
- `mode`: its task-creating capture mode;
- `range`: the item's half-open UTF-8 byte range in the original draft;
- `marker_at`: the UTF-8 byte offset where an action marker can be inserted
  on the parent line before a trailing Pomodoro suffix;
- `route_range`: the item's own destination-token range, omitted when absent;
- `discard_range`: the item and one adjacent blank-line separator, omitted
  when it has authored children;
- `matches`: ranked candidate rows;
- `linked`: resolved candidate keys already referenced by this item, with
  `link` set to `relation`, `dependency`, `blocked_task`, or `parent`.

Every range is half-open and indexes UTF-8 bytes in the exact draft passed to
Bob. `marker_at` and ranges are splice anchors for that draft only; clients
must discard them after an edit.

A match row contains `key`, `band`, clean `title`, `matched_ranges`, `kind`,
`status_symbol`, `status_name`, `status_type`, nullable `closed`, `route`,
`locator`, `note_path`, `target`, `line`, `ref`, nullable `block_id`,
`requires_block_id`, `block_id_suggestions`, `actions`, and `default_action`.
`matched_ranges` are half-open `[start, end]` UTF-8 byte pairs in `title`.
`closed` is `null` for an open task or `{ "status": "done" | "canceled", "date": "YYYY-MM-DD",
"days_ago": N }`. `kind` is `task`, `project`, or `reading`. `key` is the
stable note-path-and-block-ID identity, or a digest for an ID-less task.
`target` and `line` can be passed to Obsidian's task Go To URL; `ref` is the
stale-safe task reference.

Rows with block IDs include Bob-authored `replacements` for `relation_to`,
`parent`, `dependency`, and `blocked_task`. ID-less rows omit `replacements`,
set `requires_block_id: true`, and include `route`, `note_path`, `ref`, and
suggested IDs; the existing `capture-task-id` flow supplies replacements
after the user chooses or enters an ID.

`actions` contains only edits that Bob verified by applying the replacement
to a copy of the draft and parsing it again. Open rows may offer `relate`,
`note`, `needs`, and `blocks`, plus `open` and `discard` when their anchors
allow those actions. Closed rows offer only `open` and `discard`. The default
is `discard` for an exact title, `note` for a possible duplicate, `relate`
for a similar or held row, and `open` for a non-exact closed duplicate.
Return remains the capture submit action; these are explicit draft edits and
never create a link by themselves.

```bash
bob capture-related -b ~/bob -f json -- 'Install new UPS battery @dev'
bob capture-related --query 'Install UPS battery' --format json
```

The first command emits draft item anchors and actions but no score. The
second uses the evaluation harness and adds a score to each match. Human
draft output groups matches by item and shows their status, highlighted title,
band, and locator. It omits empty bands entirely; there is no “no duplicates”
row. Unreadable candidate notes appear as bounded `warnings` while other
matches remain available. Usage errors exit nonzero and JSON errors use the
usual `{"ok": false, "error": "…"}` envelope.

## `bob capture-rewrite`

```bash
bob capture-rewrite [-c|--cursor N] [-f|--format human|json] [--] [TEXT]...
```

Applies the capture grammar's automatic draft rewrites -- template start, the
block-ID separator toggle, the relation-label pipe attach, and the bare `@@`
absorption rule -- and reports the
resulting edits, cursor, and a human summary. It is purely lexical and
completely read-only: it never opens the vault, never reads the clipboard
or the URL routing config, never touches the filesystem, and takes no
`--bob-dir`. `bob capture-parse` reads that config unless `--no-ref` is
set; see [above](#bob-capture-parse).
If `TEXT` is omitted and stdin is piped, it reads the complete piped stdin
stream. Missing `TEXT`, invalid options, or a cursor outside `TEXT` or
inside a UTF-8 character return exit 2. Other draft text succeeds, with
`changed: false` when nothing needed to change. Exact-name `#"…"` literals
are never rewritten: they keep their quotes and spelling byte-for-byte.

Template drafts are never rewritten: `capture-rewrite` returns
`changed: false`, with no rule or edits, so it cannot absorb `@@` or toggle a
template marker.

**Template start.** Typing a third `@` at byte 3 when the first physical line
is exactly `@@@` replaces it with `@@template:` and a line ending. The existing
LF or CRLF is reused, and the cursor moves to a fresh blank line. This typing
gesture requires `--cursor`; without it, the rule is skipped.

```bash
bob capture-rewrite -c 3 -f json -- '@@@'
```

The result is `@@template:\n` with cursor 12 and summary
`Started a @@template: draft`. If the draft already has following lines, they
move down so the cursor still lands on a blank line. Feeding the result back
through the command is a no-op.

**Block-ID separator toggle.** Typing `^` immediately after a complete
plain `@route:id`, or `:` immediately after `@route^id`, consumes that
character and swaps the separator. `--cursor` is required: only the token
ending at that caret is examined, so the command never searches the draft
for a toggle. The new key must sit against the ID with no intervening
whitespace and must end the token (end of input or a following whitespace
boundary). Inserting it inside an ID does not toggle a prefix.

```text
Do work @file:id^
```

becomes `Do work @file^id`, with the cursor placed just past the unchanged
ID. The same gesture the other way restores the colon spelling. Solo
markers, leading markers, a trailing marker before later body text, a
trailing marker on an authored child line, and one item in a blank-line
batch are eligible wherever the shared grammar already recognizes that
token as the item's destination. Route and ID spelling, surrounding
whitespace, CRLF, schedule/priority/dependency markers, and other items
stay byte-for-byte. Suffix families (`#name`, `+`, `=...`), `@@`, `@!`,
standalone `^route:id`, `&note:id`, `!note:id`, project-task `:id`/`^id`,
incomplete or malformed markers, same-separator repeats, and
marker-looking prose the contextual parser does not select are left
unchanged. This is an editor assist: `bob capture` and `bob capture-parse`
keep the literal `@file:id^` text.

**Relation-label pipe attach.** Typing `|` after whitespace that follows a
relation token deletes that whitespace so the pipe glues to the token.
`--cursor` is required: only the `|` token ending at that caret is examined,
so the command never searches the draft for a pipe. The run before the pipe
must be spaces or tabs (never a newline) with a non-whitespace byte on the
same physical line before it, and the glued draft must be a place where
bob's completion grammar would open a `relation_label` field, so prose
pipes, mid-line relations, already-labelled tokens, partial targets,
escapes, and newlines are left unchanged.

```text
Fix trip ~work:foo |
```

becomes `Fix trip ~work:foo|`, with the cursor placed just past the glued
`|`. The same gesture works for `~~` targets, quoted notes such as
`~"My Note":target`, leading runs, and tokens before later body text. This
is an editor assist: `bob capture` and `bob capture-parse` keep the literal
` |` text. Feeding the result back through the command is a no-op,
because no whitespace is left before the `|`.

Typing a bare `@@` inside an item that already carries a local destination
marker moves that marker onto the `@@` and deletes it, so the item ends up
declaring its own existing destination instead of shadowing it:

```text
Buy milk @dev @@
```

becomes `Buy milk @@dev`, with the cursor placed just past the rewritten
token. The absorbed marker can be on any of the item's lines, not only the
one the `@@` was typed on:

```text
Buy milk @dev
- more detail @@
```

becomes `Buy milk\n- more detail @@dev`. When the item has no local marker
of its own but the draft already carries exactly one other `@@` declaration,
that declaration's payload moves onto the bare token instead:

```text
@@foo
Buy milk @@
```

becomes `Buy milk @@foo`, and the now-empty declaration-only line is
deleted, terminator included. Either way, every *other* `@@` declaration
token still left in the draft is also deleted, so a rewritten draft never
ends up with more than one declaration. Deleting a token also consumes one
adjacent whitespace run so no double space is left behind, and deleting a
line's only token deletes the whole physical line.

**Which bare `@@` claims the rewrite:** the one containing or ending at
`--cursor`, when a cursor is given; otherwise the last bare `@@` in source
order. A rewrite is idempotent: running it again on its own output is a
no-op, because the claiming token is no longer bare.

An item's single local marker that cannot be expressed as a declaration --
`@route#Section`, `@route+block-id#section`, `@route^block-id`,
`@route:block-id`, `@route^block-id+` as a project note,
`@route+block-id` as a task toggle, a solo `@route:block-id…` / `^route:block-id…`
Pomodoro link, a `x[<N>][!<M>]` Pomodoro close (`pomodoro_close` is non-absorbable and
close items are never rewritten), or a trailing bare `#`
-- is left untouched; the result reports `changed: false` plus a
`notices` entry naming the marker and why, e.g.
`@@ cannot take a section: leave @notes#Ideas on this item, or delete it and declare @@notes`.
For a task toggle the notice is
`@@ cannot take a task toggle: leave @cash+goog-exit on this item, or delete it`,
for a project note it is
`@@ cannot take a project note: leave @cash^goog-exit+ on this item, or delete it`,
and for a Pomodoro link it is
`@@ cannot take a Pomodoro link: leave ^sase:deep-fix on this item, or delete it`.
`^` items are never rewritten. An item with more than one local marker is also left untouched, with no
notice, because `bob capture-parse` already reports that duplicate as a
`duplicate_capture_marker` diagnostic.
An item with more than one local marker is also left untouched, with no
notice, because `bob capture-parse` already reports that duplicate as a
`duplicate_capture_marker` diagnostic.

Absorption is an editor typing assist, not a grammar rule: `bob capture`
still executes exactly the text it is given, so a bare `@@` there is still
an incomplete declaration and still fails. Run `bob capture-rewrite` first
and feed its `text` back through `bob capture` (or into the draft the user
is editing) to apply the assist.

JSON output is a single versioned object:

```json
{
  "ok": true,
  "schema_version": 1,
  "input": "Buy milk @dev @@",
  "text": "Buy milk @@dev",
  "changed": true,
  "cursor": 14,
  "rule": "absorb_local_marker",
  "edits": [
    { "range": { "start": 9, "end": 14 }, "replacement": "" },
    { "range": { "start": 14, "end": 16 }, "replacement": "@@dev" }
  ],
  "summary": "Moved @dev into @@dev",
  "notices": []
}
```

`rule` is `start_template`, `switch_block_id_separator`,
`attach_relation_label`, `absorb_local_marker`, or `absorb_declaration`,
omitted when nothing changed. `cursor` is present only when `--cursor` was
supplied and is mapped through every edit. For `start_template`, it lands at
the start of the fresh line; separator toggles land just past the unchanged
ID, pipe attaches land just past the glued `|`, and absorption lands just
past the rewritten `@@<payload>` token.
`edits` index `input`, are sorted by `start`, never overlap, and applying
them left-to-right yields `text`; `text` always equals `input` when
`changed` is `false`. `summary` is omitted when nothing changed; `notices`
is omitted when empty. Human output prints a header naming the rule, the
before/after draft, the summary, and any notices; when nothing changed it
prints a dim `no rewrite` line plus the notices.

## `bob capture-complete`

```bash
bob capture-complete --cursor BYTE [-a|--all-tasks] [-b|--bob-dir DIR] [-f|--format human|json] [--] [TEXT]...
```

Returns cursor-aware completion candidates for in-progress capture `TEXT`. It
shares the phase-grammar tokenizer and `@token` classification with
`bob capture-parse`, so a completion can never disagree with the marker
highlighting derived from that command; it never independently reparses marker
prefixes. Shell completion is another thin client of this service: `TEXT` on
`capture`, `capture-parse`, and `capture-rewrite` completes markers through
the same extraction (see `docs/completion.md` Capture markers). `--cursor`/`-c` is required and must be a UTF-8 byte offset on a
character boundary within `TEXT`. It is not the same flag as `bob capture -c` /
`--clip`. A missing `TEXT` defaults to an empty draft rather than an error,
since cursor `0` against an empty draft is an ordinary interactive state, not a
mistake.

For a blank-line-separated batch draft, completion always scopes to the item
and physical line the cursor is on: only that item's first (parent) line
offers a leading marker, matching `bob capture-parse`'s leading-wins
precedence, and a later valid column-zero or two-space nested authored line
only completes its own trailing marker. A cursor sitting on a separator row,
a later line's source indentation, `-`/`*`/`+` bullet marker, or marker
separator is never completable. Orphaned nested lines do not provide
completion.

The service itself decides whether completion applies. An unrecognized
marker, a cursor sitting in plain body text, or a cursor on an `@token` that
is not the leading or trailing marker on its line all return a successful
empty result rather than an error. A lone leading `@route` fragment with no
body text yet is still completed on the parent line, even though
`bob capture` would leave that exact input literal.

Route completion covers a bare `@`, a still-typing `@fragment`, and the
missing route portion of `@^...`, `@+...`, `@:...`, and `@#...`, plus the route
component of a `@@`, `@@fragment`, or `@@route+...` declaration anywhere in the
draft. Replacement ranges for that declaration exclude both `@` sigils and the
`+`. Parent-task
completion covers `@@route+fragment` the same way it covers `@route+fragment`,
including `--all-tasks` missing-ID candidates. An inherited global route also
becomes the current note for same-note wikilink heading/block completion unless
that item overrides it. Route completion is backed by the same
scan as `bob capture-targets`. For a template draft, completion runs against
the masked template: the header, rows, and placeholders return empty success,
while route, section, Pomodoro-name, dependency, and wikilink pickers in the
template body remain available. Block-ID contexts and replacements that
overlap a placeholder return empty success. On `@@`, `@@te`, and other
first-nonblank-line prefixes of the reserved header, route completion offers a
`template:` candidate (`kind: "template"`, label
`Bulk template: one capture per row below`), first for a matching query and
last for an empty query. It is not offered on later lines, even when a later
line repeats the same prefix text. Section completion
covers `@route#prefix`,
backed by the same scan as `bob capture-sections`. Task-section completion
covers `@route+id#prefix` and a bare `@route+id#` once the item has body text,
backed by the same scanner as `bob capture-task-sections`; the candidate
`replacement` is the section slug (`future-work` for `FUTURE WORK`).
Pomodoro-name completion covers `@route:id#prefix`, a bare `@route:id#`,
`@route:#prefix`, `@route:id=name` (e.g. `@work:outline=bu|`), and `@route+id#prefix` or a bare `@route+id#` while the item
has no body text; only the route
must already resolve because the Pomodoro list does not depend on the block
ID. On a `@<route>:<block-id>[#<name>]=<X>` marker the `=<X>` suffix is never
a completion field when it is timing/close: block and name replacement ranges end before the `=`, a
cursor inside a timing suffix returns an empty success, and accepting a candidate
preserves the typed suffix. A cursor inside a suffix name (`=name`) is the `pomodoro_name` field described above. The `=x[<N>][!<M>][~<K>]` close suffix behaves the same way: it
is never a completion field, replacements still stop before `#`/`=`, and a
cursor anywhere inside the suffix, including the task-number lists and a
dangling `,`/`!`/`~` separator, returns an empty success. A whole-item `+[N]`/`-[N]`
Pomodoro adjustment, `++[N]`/`--[N]` Pomodoro shift (a bare `+`, `-`, `++`,
or `--` is one unit), `x[<N>][!<M>][~<K>]` close, or `=`/`=<X>` start (a bare `=` starts
25 minutes) is an action and requests no route or
task completion candidates: a cursor on such an item returns an empty success. Work Log text completes like a bullet line whether it sits below the close or on it. A
whole-item `=name` named start instead completes the name as
`pomodoro_start_name`, per token inside chains: `=bu|`, `x =bu|`, and `x wired it =bu|` report `pomodoro_start_name` with query `bu` and replacement over the name only, just like `=#bu`; a cursor inside the name part
completes it, while a cursor on `=<X>` or at the `#` byte itself returns an
empty success. The name is located through the lexer's recorded offset and marked bare-spelled (e.g. a `bare_name` bool); any candidate whose slug fails `can_spell_bare` gets the replacement `#<slug>`, so accepting it yields a valid `=#2fa` (substring matches matter). A cursor anywhere inside a drop part, including a dangling `~`/`,`, returns an empty success. Near misses complete the same way valid tokens do. The name `replacement` covers only the name part (it ends before `~`), so accepting
a candidate preserves the typed `=<X>#` and any typed `~<K>` list. Candidates
follow [PT7 menu composition](pomodoro-themes.md#pt7-menu-composition):
matching Today rows first, Recent exact/prefix matches, a create row when no
Today or Recent name matches exactly, Recent substring matches, then utility
rows. Today rows keep their existing order: planned placeholders (with
`next_up` on today's next future entry), then `again` rows for completed
sessions, most recent first. Utility rows are the unnamed “Name it” entries
(`requires_name: true`) followed by the running session. They are never
filtered out, so `=#zzz` still offers creation before those utilities.
Recent rows exclude hidden themes and components of any Today row before
query filtering. Empty queries show at most 5 Recent rows; other queries show
at most 12. New, Recent, and again rows carry `plan_themes_after` and
`plan_themes_cap`. Recent and again replacements preserve the displayed name:
a slug that loses spaces or selects another name is replaced with an exact
quoted literal, including `#` when the input uses a bare start. A named-start name field keeps context `pomodoro_start_name` and its
candidates when exactly one timed session runs. The object is reported as soon
as the name field exists, including an empty `=#` or `=3#`, so a picker can
describe the swap before a name is typed. The additive top-level
`override` object then previews that state: it is present only when no earlier
session operator in the draft changes the ledger, `keeps_ledger` is true when
the typed `<X>` suffix is empty and false for fresh timing, and `running`
carries the current session (`pomodoro_name`, omitted when unnamed, the
1-based day-file `line`, and its `time_range`, such as `0920-0945`). An
unnamed running session with Task Links also carries the derived R9 `homes`
object (forecast); a named one omits it. The whole `override` object is
omitted unless exactly one timed session runs. This context is state-based
and applies to ordinary `=name` as well as counted `=3#name` fields; a
preceding close in `x =name` means there is no running session to preview. A cursor inside a link suffix name (`@work:outline=bu|`, `^work:outline=bu|`) yields `pomodoro_name` with the route and block ID, a query from the name start to the cursor, and a replacement over the name only; the same `#<slug>` fallback applies, so `@work:outline=#2fa` is valid. A partial literal (`=#"DEEP|`, `@r:id#"SASE V|`) is the same Pomodoro-name field with the query decoded (`DEEP`, `SASE V`) and the replacement covering the whole partial literal including its opening quote, so accepting a candidate keeps one shell-quotable token under the existing single-quote rule.
For example, with `CAPTURE` running at line 5:

```json
"context": "pomodoro_start_name",
"override": {"keeps_ledger": true,
             "running": {"pomodoro_name": "CAPTURE", "line": 5,
                         "time_range": "0920-0945"}}
```

The `pomodoro_name` context uses the same groups, with open named ledger
entries as Today rows and unnamed entries as utilities. Matching Today rows
keep their prefix-then-substring order. Entries with the same normalized name key collapse
to the first row with `match_count` reporting the number of matching entries.
Nameable rows use an empty `replacement`; clients prompt for a name rather
than inserting it. A theme completed today is Recent in this link context
and an “again” row in the start context.

Accepting a creation or Recent row edits the draft only; the later
`bob capture` transaction performs the write with its normal placement,
missing-section, multiple-running, and strict-budget checks. A new create row
requires a valid name and a ledger that can place it. Recent rows carry
`origin: "history"`, `last_used`, and `days_used`, with no `ref` or `line`.
An exact Recent match serves as the create row. A hidden exact name remains
typable and is marked `hidden: true` on the create row. A new create row can
also carry up to two `similar_to` names as a quiet near-duplicate heads-up;
this never changes order or Return. Completion and acceptance never write
theme preferences. Shell descriptions include `recent · Oct 21 · 3 days`.

Pomodoro block-ID
completion covers `@route:prefix` and parent-task completion covers
`@route+prefix`; both are backed by the same open-task scan as
`bob capture-tasks`. By default both contexts only offer tasks that already
carry a block ID so older callers stay compatible. Pass `-a`/`--all-tasks` to
include open tasks that still need an ID, but only in the `task` / `@route+`
context. Pomodoro `@route:` completion stays identified-only even when
`--all-tasks` is set. Missing-ID discovery is therefore opt-in and
plus-context-only. For `link` intent, `@route:` candidates are further
restricted to statuses the link path accepts (Ready, Blocked, Next, In
Progress), carry an additive 1-based `line` and a nullable `pomodoro` object
with the exact `active_task` shape, and rank exactly as before (document order
on an empty query). For `new` and `project_note` intents, `candidates` is
`[]`: every existing ID would be a duplicate-ID error.
The right-hand side of `@route^block-id` completes as `task_block_id` once
the route resolves, with empty `candidates` and the additive `block_id`
object below. A project-note `+` directly after either block-ID part is the
sigil, never part of the replacement: `@sase^x+` at cursor 7 returns
`task_block_id` with replacement `{6, 7}`; `@sase:x+` returns no context, a cursor after the sigil (and before `#` for `^`) returns an empty
success, and a `+` after `#name` stays part of the Pomodoro name. A solo
leading `^` token instead completes active tasks: while the
cursor is in the `route:block-id` part (including an empty part) the context
is `active_task`, offering In Progress and Next tasks with block IDs,
backed by the active-task discovery scan — queued tasks in ledger order,
then unqueued In Progress, then unqueued Next — with prefix matches before
substring matches over `route:block-id`, the block ID, the task text, the
section, and the Pomodoro name, so `^dee`, `^sase:dee`, and `^outline` all
find their tasks. `replacement` runs from just after `^` to the end of that
part and always stops before `#`/`=`, so typed suffixes survive an accept,
and `query` is the text from after `^` to the cursor. A `#name` after
`^route:block-id` completes Pomodoro names exactly as it does after
`@route:block-id`, and a cursor inside `=<X>` or `=x[<N>][!<M>][~<K>]` returns an empty success. A solo
leading `:` token completes `task_link`: while the cursor is anywhere in
`[token.start, token.end]` — including just before the sigil, so clients can
refetch the full list at `replacement.start` — the context offers every
linkable open task (Ready, Blocked, Next, In Progress) in the routable inbox,
area, and non-terminal project notes, in canonical picker order and ranked by
the query, with ID-less tasks always included (`--all-tasks` does not affect
this context). The `replacement` covers the whole `:` token, sigil included,
because accepting rewrites the query into the canonical `@route:block-id`
link, and `query` is the token text between the sigil and the cursor (empty
at or just after the sigil). For example `bob capture-complete -c 1 -- ':'`
lists the full routable task catalog, and `-c 4 -- ':dee'` narrows to matching
rows with `replacement` `{0, 4}`.
A whole-item `!` token completes `task_complete`: while the cursor is anywhere in
`[token.start, token.end]` — including just before the sigil, so clients can
refetch the full list at `replacement.start` — the context offers every open
task (Ready, Blocked, Next, In Progress) in the vault-wide completable catalog.
The `replacement` covers the whole `!` token, sigil included, because accepting
rewrites the query into the canonical `!note:block-id` completion, and `query`
is the sigil-decoded text between the sigil and the cursor (empty at or just
before the sigil). The response carries a vault-scoped `picker` descriptor
(`kind: task_complete`, `scope_token: "!"`, both ranges the token); a bare
whole-item `!` additionally returns the `!` and `[` `action_continuation_keys`
so an editor can hand `!!` and `![[` straight back to prose and embeds.
Candidates carry the `!loc:id` `replacement` (empty for ID-less and guarded
rows, which resolve through `capture-task-id`'s `complete_replacement`
instead), the exact `note_path`, the display `locator`, the `today` /
`in_progress` / `next` / `open` group, and — for today rows only — the `today`
placement: `role` (`running`, `worked`, `queued`, or `noted`), the entry
(`line`, `name`, `time_range`, `status`; omitted for `noted`), and the
distinct-session count. Any unstruck block link — plain or embedded, alias
allowed, `🍅` markers stripped — places its task when it sits outside fenced
code and Depends-On lines and resolves to an open task: links under the
running entry read `running`, under a completed entry `worked`, under any
other open entry `queued`, and outside `## Pomodoros` `noted`, keeping the
strongest role. Recurring tasks and tasks another `!` item in the draft
already names stay visible with `recurring` / `already_selected` and a
`disabled_reason` but no insertable replacement. Empty queries list today rows
first (running in ledger order, then worked most-recent-first, then queued in
ledger order, then noted in document order), then In Progress and Next by note
path and line, then all other open tasks; hidden rows sink within their
section and recurring rows sink last. A nonempty query keeps today matches on
top by score, then all other matches by score, using the shared tiered matcher
over the task text, locator, `locator:block-id`, block ID, note path, section,
and today Pomodoro name. For example `bob capture-complete -c 1 -- '!'`
lists the full completable catalog, and `-c 4 -- '!fix'` narrows to matching
rows with `replacement` `{0, 4}`.
A `*` modifier completes `blocked_task` with the same `query`, `owner`, and
replacement-range semantics as `task_dependency`: the replacement covers
exactly the active modifier (sigil and quoted note included), including
just before the sigil so clients can refetch at `replacement.start`.
`X *`, `X *jo`, and `X *job:` all open this context. The pool is open,
editable tasks only (daily notes, `done/`, `_templates`, `_generated`,
`_conflicts`, dot-directories, and fenced code are absent). Rows reuse
the `DependencyCandidate` shape plus `already_blocked` (omitted when
false) and `disabled_reason` (`the owner task itself`, `would create a
dependency cycle`, `a project cannot depend on its own resident`, or the
existing duplicate-block-ID reason). With an empty query, ranking is
today's linked tasks (the running entry's first), then Pending `[/]`,
Next `[*]`, the owner's note, then everything else; a non-empty query
ranks by match score first, then that order. Replacements use `*`
(`replacement_for_sigil`). ID-less and guarded rows resolve through
`capture-task-id`'s `blocked_task_replacement`. Shell completion adds a
`blocked tasks` group. See
[Blocking existing tasks with '*'](#blocking-existing-tasks-with).
For `task_dependency`, `blocked_task`, `relation_to`, `relation_from`,
and `relation_label`, `owner` remains the lexical
`dependency_target`; when it resolves to an existing task, it also carries
the additive `text`, `note_path`, `status_symbol`, and `status_name` display
fields. A `new_task` owner carries its non-empty draft `text`. These additions
are omitted when absent, and responses for drafts without relationship
modifiers remain byte-identical.

`~` / `~~` add the `relation_to` and `relation_from` contexts and the
`relation_label` context. `relation_to` uses the whole `discover`
catalog; `relation_from` offers open editable tasks, with read-only
sources kept visible and disabled. Empty-query order for `relation_to`
is the owner's note, In Progress, Next, other open tasks, closed tasks
in active notes, then `done/`. For `relation_from`, today's linked tasks
(the running entry first) precede In Progress, Next, the owner's note,
and other editable tasks. A query ranks by match score first, then these
tiers. Rows carry `already_linked`, `existing_label`, and the information
badge `also_prerequisite` plus `disabled_reason` (`the owner task
itself`, the duplicate-block-ID reason, `archived tasks can't get an
id`, and — for `relation_from` only — `read-only note` and `its LINKS
line is malformed`). Label suggestions scan every LINKS line, including
`done/` and lines beneath ID-less tasks, and return
`suggestions[] {label, count, replacement}`. A non-empty typed label adds
a first `{label, typed: true, replacement}` row with bob's canonical
`|label` / `|"multi word"` spelling; invalid labels carry a
`disabled_reason` naming the broken label rule. `relation_label` responses
also carry an additive `relation` object: `direction` is `to` for `~` and
`from` for `~~` (spelled like `capture-parse` `relations[].direction`), and
`range` runs from the sigil's first byte to the `|` separator, so
`range.end` always equals `replacement.start` -- the quoted-note form is
covered too. The lexical `owner` covers `relation_label` as well, with the
same resolved display fields as the relation target contexts. No other
context carries `relation`. `picker.relationship`
gains `relations[]` (direction, text) for the owner picker, and shell
completion gains `links to` / `linked from` groups. `capture-task-id`
returns `relation_to_replacement` and `relation_from_replacement` for
the Add block ID flow. See
[Linking tasks with '~' and '~~'](#linking-tasks).

The analogous parent-task picker accepts a terminal `+query` on an item's
parent line or an eligible authored child line. It uses context `task_parent`,
strips `+` from the top-level `query`, and replaces the full selector token.
It lists linkable open tasks in that same routable catalog, including ID-less
tasks whose empty `replacement` must not be inserted; those rows include block-ID
suggestions for the Add block ID action. A lone `+` keeps its Pomodoro
adjustment meaning in parsing but completion returns this picker with
`action_continuation_keys` (`0`–`9` and `+`). Scoped `@route+id` completion
continues to use context `task` and replace only the ID. When the current item
has complete `&` or `*` modifiers, the parent picker descriptor adds
`relationship.dependencies[]` and `relationship.blocked_tasks[]`, each with
the typed `raw` token, `note_path`, `block_id`, and resolved `text` when
available. This metadata appears for both the vault-wide `+` picker and the
scoped `@route+` picker. Candidate `disabled_reason` values identify the
prerequisite itself, blocked task itself, a dependency cycle, or a project
depending on its own resident; guarded rows have no insertable replacement
and shell completion skips them.
`@route:` (`pomodoro_block_id`) completion now carries the `block_id` object
and link-only filtering above; an empty-query marker-only `@route:` still
lists that note's linkable tasks in document order. An empty block-ID component (`@route+#`) returns a successful empty
task-section list. An unresolvable parent task returns a successful empty list
plus one bounded `warnings` entry; the warning names the route and block ID
without logging draft text or the task description.
Route, section, and wikilink candidates rank exact prefix matches before
substring matches, case-insensitively, while keeping each discovery source's
stable order. Task-section candidates rank slug-prefix matches first, then
slug-substring matches, in document order inside each tier. Task candidates in `@route+` search block ID (when present),
task text, section, and status name or symbol the same way, but identified
tasks always stay ahead of unidentified tasks and prefix matches precede
substring matches inside each of those two groups. A non-matching candidate
is dropped, and an empty query keeps every eligible candidate.

When the cursor is inside a valid Obsidian wikilink component, link completion
takes precedence over marker completion so `@` and `%` inside link text remain
ordinary link text. `wikilink_note` searches Markdown note paths, stems, and
frontmatter aliases. `wikilink_heading` searches ATX headings in a resolved
target, in the current capture destination for `[[#...]]`, or across the vault
for `[[##...]]`. `wikilink_block` searches named block IDs in the analogous
target, current-destination, or `[[^^...]]` vault-wide scope, except on a
Work Log bullet line under a whole-item `x`, where block candidates are
suppressed (entries reject block links) while note and heading completion
keeps working. The note index is
read-only, skips hidden directories plus `.git`, `.obsidian`, `_generated`, and
`_templates`, never follows directory symlinks, and returns bounded warnings for
individual unreadable notes or malformed alias frontmatter while keeping path
completion available.

JSON output is a single versioned object:

```json
{
  "ok": true,
  "schema_version": 1,
  "cursor": 3,
  "replacement": { "start": 1, "end": 3 },
  "context": "route",
  "candidates": [
    { "replacement": "cash", "route": "cash", "label": "cash.md", "kind": "area", "status": null }
  ]
}
```

`replacement` is the half-open UTF-8 byte range a chosen candidate replaces in
full, regardless of where the cursor sits inside it; it is always present, even
in an empty result, where it collapses to a zero-length range at the cursor.
`context` is `route`, `section`, `pomodoro_block_id`, `task_block_id`, `project_task_block_id`, `pomodoro_name`, `task`,
`task_section`, `active_task`, `task_link`, `task_goto`, `task_parent`, `task_dependency`, `blocked_task`, `task_complete`, `wikilink_note`, `wikilink_heading`, `wikilink_block`, or `null` when no completion field is
active. `task_parent` covers a terminal `+query`, strips the sigil from its
additive top-level `query`, and replaces the whole selector. `task` covers
scoped `@route+id` with only the ID span replaced and the typed ID query.
`query` is present only for `task_parent`, `task`, `task_dependency`, `blocked_task`, `task_complete`, `relation_to`, `relation_from`, and `relation_label`; it
contains the selector text after its sigil (and a decoded locator for a
dependency, blocked-task, or relation query). `blocked_task` is the `*` twin of
`task_dependency`: same owner and replacement-range rules, `*` replacements,
and the open-editable pool documented under
[Blocking existing tasks with '*'](#blocking-existing-tasks-with). Both parent-task contexts include an
additive `picker` descriptor: `kind` is `parent_task`; `scope` is `vault`
with `scope_token: "+"` for the bare selector, or `note` with the canonical
`@route+` / `@@route+` scope token for scoped selection. `note_target` is
present only for note scope. `marker_range` covers the selector (or the
scoped marker through its ID), and `trigger_removal_range` covers the full
bare selector or only the `+` through the scoped ID. All offsets are
half-open UTF-8 byte ranges. Only the exact lone `+` includes
`action_continuation_keys` (`0` through `9`, then `+`). The `task_complete`
context carries its own vault-scoped `picker` descriptor (`kind:
task_complete`, `scope_token: "!"`) instead; the descriptor is omitted from
all other contexts. `task_block_id` covers `@route^prefix` once the route resolves;
`candidates` is always `[]`. `project_task_block_id` covers a cursor inside a
trailing ` :` / ` ^` task ID token on a first-level bullet of a project-note
item with a resolved route and block ID, from just after the sigil to the
token end; the replacement is the ID span, empty at the cursor for a lone
sigil. All three block-ID contexts carry an additive
top-level `block_id` object with `route`, `relative_target` (for example
`sase.md`), `note_exists` (false for a missing note, which is not an error),
`marker` (`:` or `^`), `marker_range` (the whole marker token's draft-global
UTF-8 byte range, for highlighting), `intent` (`link`, `new`, or
`project_note`), `body` (the item's normalized parent-line body, or `""`),
`allowed_character` (a one-character regex: `[A-Za-z0-9-]` for both `:` and
`^`), `allowed_description` (the human wording from the
matching validator), `suggestions` (at most 3, deterministic, for `new` and
`project_note` intents with a non-empty body), and `used` (every ID the
duplicate check sees, deduplicated by first occurrence in document order with
1-based `line`; task lines carry `task: true` plus `status_symbol`,
`status_name`, and the description as `text`, while other lines carry
`task: false`, null status fields, and the line with its list marker and
trailing `^id` removed, trimmed to 160 characters; `[]` when the note is
missing and for `project_note` intent). Intent is `project_note` when `+`
follows the ID part; `^` is otherwise always `new`; `:` is `link` exactly when
the whole-item parse would classify the item as `pomodoro_link` once a valid
ID fills the part (marker-only items), else `new` (items with text, `s:`/`$<N>`
/`%` markers, child-line and batch positions behave the same way). The
`project_task_block_id` object always carries intent `new` with empty
`candidates`: `route` is the project-note stem (for example `cash_goog_exit`
with `relative_target` `cash_goog_exit.md`), `marker` is the typed sigil
(`:` or `^`), `marker_range` is the sigil plus the ID, `body` is the bullet
body without the ID or a leading checkbox, and `used` holds `prj` (with that
bullet's project body as `text`) plus every other task ID already typed in
the item. A `used` entry carries `line` (1-based physical draft line),
`task: true`, null `status_symbol` / `status_name`, and `text`. The object
reads no routed note except to set `note_exists` for the project note.
Suggestions derive from the task text: the first phrase with a word (code
spans, quoted phrases with mixed `"`/`“` quotes, parentheticals, and
`[[wikilink]]` alias-or-target, in text order) as its first 4 words, then the
first 3 prose words, then — when the first prose word is a leading verb — the
next 3 words; words are ASCII alphanumeric runs, lowercased, minus stopwords;
joined with `-`, truncated at a word boundary to at most 32 bytes, validated,
deduplicated, and suffixed with the first free `-2`…`-9` when taken (dropped
when none is free). For example `Fix flaky gkeep test` suggests
`fix-flaky-gkeep` and `flaky-gkeep-test`. Older clients without the
`block_id` object treat `pomodoro_block_id` as a link list and see no picker
for `task_block_id`; the app never falls back to the inline list for these
three contexts. Older clients route the unknown `project_task_block_id`
context to their inline list, which has no candidates, so they degrade
silently. Each candidate's `replacement` is the exact text to insert; wikilink
candidates also include `cursor_after`, the post-accept UTF-8 byte offset after
deduplicating or synthesizing the closing `]]`. A route candidate has `route`,
`label`, `kind` (`inbox`, `area`, or `project`), and nullable `status`. A
section candidate has `title` and `level`. A task-section candidate has
`title` (the original ALL-CAPS body), `slug`, `route`, nullable `block_id`,
`text` (the parent task description), `line`, and `child_count`; `replacement`
is the slug. A Pomodoro-name candidate has nullable `ref`, nullable `name`,
`requires_name`, optional `creates_pomodoro` (absent or false on existing
rows), nullable `line`, `state`, `status_symbol`, nullable `time_range`,
`placeholder`, `is_current`, `child_count`, and `match_count`; selectable named
rows use a slug or lossless quoted literal as `replacement`, while nameable rows use an empty
replacement that clients must not insert. A create row omits `ref` and `line`,
sets `creates_pomodoro: true`, and uses the canonical selector as
`replacement`. Every named row adds `themes` (the component ThemeRefs).
Recent rows also add `origin: "history"`, `last_used`, and `days_used`.
`hidden: true` and `similar_to` are optional create-row metadata. These
additions keep `schema_version: 1`; older clients can treat Recent rows as
ordinary create replacements. A task candidate
(`pomodoro_block_id` or `task` context) has `ref`, nullable `block_id`,
`route`, `requires_block_id`, `status_symbol`, `status_name`, `status_type`,
`text`, nullable `section`, `depth`, `child_count`, additive 1-based `line`,
and nullable `pomodoro` (`line`, nullable `name`, nullable `time_range`,
`is_current`; `null` when the task is not queued or outside the link
context). Identified tasks keep
their normal block-ID `replacement`. An active-task candidate (`active_task`
context) has `replacement` (`route:block-id`), `ref`, `route`, `block_id`,
`status_symbol`, `status_name`, `status_type`, `text`, nullable `section`, and
nullable `pomodoro` (`line`, nullable `name`, nullable `time_range`,
`is_current`; `null` when the task is not queued). Human rows read
`sase:deep-fix  [*] Fix deep bug  · BUGS`. A task-link candidate (`task_link`
context) has `replacement` (`@route:block-id`, or `""` for ID-less tasks,
which clients must never insert), `ref`, `route`, `note_kind` (`inbox`,
`area`, or `project`), nullable `block_id`, `requires_block_id`,
`block_id_suggestions` (up to 3, `[]` for identified tasks),
`status_symbol`, `status_name`, `status_type`, `text`, nullable `section`,
`depth`, 1-based `line`, `group` (`queued`, `in_progress`, `next`, or
`note`), nullable `scheduled`, and nullable `pomodoro` with the exact
`active_task` shape (`null` unless the task is queued), plus
`pulls_forward: true` when set (omitted when false). Human rows read
`@sase:deep-fix  [*] Fix deep bug  · BUGS`, with the tail naming the queued
Pomodoro (`Planned` when unnamed), `In Progress`, `Next`, or the note
label, plus `· needs ID (^suggestion)` on ID-less rows and
`· scheduled YYYY-MM-DD` whenever the task carries a scheduled date. Missing-ID tasks, which appear only when
`--all-tasks` is set in the `task` context, have `block_id: null`,
`requires_block_id: true`, `block_id_suggestions` (up to 3 deterministic IDs),
and an empty placeholder `replacement` that the updated client must never
insert. A `task_parent` candidate has the same task metadata plus `note_kind`,
`group`, and nullable `pomodoro`, but no `scheduled` or `pulls_forward` fields;
identified rows use `@route+block-id` replacements, while ID-less rows have
an empty replacement, `requires_block_id: true`, and suggested IDs. Human
rows begin with `@route+block-id` or `@route+…` and describe missing IDs.
Every task-bearing candidate (`task`, `active_task`, `task_link`,
`task_goto`, `task_parent`, `task_dependency`, `blocked_task`, `task_complete`) also carries an additive
`task_kind` (`"ref"` when the row is a `#task #ref` reference reading task,
omitted otherwise, so existing JSON stays byte-identical). `bob query`
`text` also drops that `#ref` token, so `#task #ref X` reads as `X`. A
wikilink note candidate has `path`, `name`, optional `alias`,
and `match_kind`; heading and block candidates add `heading`/`level` or
`block_id`/optional `preview` metadata. Link-index warnings, when present, are
reported in a bounded top-level `warnings` array without logging draft text. A
missing note behind a resolved route or link target is not an error; it returns
an empty candidate list. Discovery failures never fall back to a default route
or an empty result silently; they return the same actionable error in human and
JSON forms as the underlying scan would.

## Discovery commands

```bash
bob capture-pomodoros [-a|--all] [-b|--bob-dir DIR] [-f|--format human|json] [-t|--tasks]
bob capture-sections --route NAME [-b|--bob-dir DIR] [-f|--format human|json]
bob capture-targets [-b|--bob-dir DIR] [-f|--format human|json] [-v|--verbose]
bob capture-task-sections --route NAME (--block-id ID | --task-ref REF) [-b|--bob-dir DIR] [-f|--format human|json]
bob capture-tasks --route NAME [-b|--bob-dir DIR] [-f|--format human|json]
```

These read-only discovery commands support interactive capture pickers. A
*route* is the canonical lowercase name for a `<route>.md` note at the vault
root; for example, route `cash` selects `cash.md`. The command-line route
options accept ASCII uppercase too and normalize it to lowercase. A picker
normally uses the commands in this order. Pomodoro notes (`text #`) skip this
list and call `bob capture` directly:

1. Run `capture-targets` and let the user choose a route.
2. For a bullet capture (`@route#`), run `capture-sections` for that route and
   let the user choose a heading. Task, sub-bullet, and Pomodoro-note captures
   skip this step.
3. For a sub-bullet capture (`@route+`), run `capture-tasks` for the route and
   let the user choose an open task. Other capture modes skip this step.
4. Optionally run `capture-task-sections` for that parent (`--block-id` or
   `--task-ref`) and let the user choose a section title, then pass
   `--task-section TITLE` to nest under that ALL-CAPS child section.
5. Run `bob capture --route NAME --section TITLE -- <text>` for a bullet, omit
   `--section` for a task, or run
   `bob capture --route NAME --task-ref REF [--task-section TITLE] -- <text>`
   for a sub-bullet.

On a successful scan, `capture-targets` returns `mac_inbox` first even when
`mac_inbox.md` does not exist, followed by top-level area notes and
non-terminal project notes, with each group sorted by route. Eligible note
filenames must already be lowercase and may contain only ASCII letters,
digits, `_`, and `-`. Area and project classification comes from YAML
frontmatter `type: "[[area]]"` or `type: "[[project]]"`; the equivalent bare
values are also accepted. Nested notes, projects whose status is `done`,
`canceled`, or `cancelled` (case-insensitively), and other note types are
omitted. Human output groups routes by kind. JSON output has `ok`, `bob_dir`,
`count`, and an ordered `targets` array; each target has `route`, `name`,
`label`, `kind`, `is_default`, `status`, `relative_path`, and
`project_name_aliases` (the note's external names, `[]` when it has none;
human output shows them as `aka bob-cli`). `--verbose`
reports top-level Markdown files omitted because their filename is not a valid
route, plus `project_name_aliases` problems (malformed entries, duplicate
claims, and shadowed aliases); other omissions remain silent. See
[`docs/projects.md`](projects.md) §"External names (`project_name_aliases`)".

`capture-sections` lists each parsed ATX heading (H1-H6) except a heading
titled exactly `Tasks` and the generated `Next & In Progress`, `Blocked`, and
`Done & Canceled` status-group headings below a `Tasks` heading, in document
order. It ignores headings in YAML
frontmatter and fenced code blocks. Route input is normalized to lowercase,
and a missing note successfully returns an empty list. JSON output has `ok`,
the normalized `route`, `count`, and an ordered `sections` array whose entries
each have `title` and `level`.

`capture-tasks` lists open Obsidian Tasks entries in document order, including
indented sub-tasks. Done and canceled tasks are omitted; Todo, In Progress, and
On Hold statuses are included, and an unknown status symbol is treated as an
open Todo. Route input is normalized to lowercase, and a missing note
successfully returns an empty list. Human output groups tasks beneath their
nearest ATX heading and shows status, block ID, and status name without emitting
color escapes when piped. JSON output has `ok`, `route`, `relative_target`,
`count`, and an ordered `tasks` array. Each task has `ref` (`<line>:<digest>`),
`line`, nullable `block_id`, `status_symbol`, `status_name`, `status_type`,
`text`, nullable `section`, indentation `depth`, and `child_count`. The ref is
the picker-safe value accepted by `bob capture --task-ref` and
`bob capture-task-id --task-ref` and can recover when unrelated edits shift
the task's line.

`capture-pomodoros` lists Pomodoro ledger entries from today's daily note. The
daily note is selected exactly as `bob capture` selects it: `BOB_DAY_FILE` when
set and nonempty, otherwise `<bob-dir>/YYYY/YYYYMMDD.md` from `BOB_NOW` or the
local date. Open entries are listed by default; `--all` includes completed
entries. A missing daily note or a missing `## Pomodoros` section returns a
successful empty list with one warning naming the file, so picker callers can
degrade to "nothing to choose" without showing an error dialog. When the day
has more than one open timed Pomodoro, no entry is marked current and the
result includes a warning.

Human output shows one row per entry with the name, selector slug, time range
or `planned`, and `current`, `completed`, child-link-count, or `empty` badges,
without emitting color escapes when piped. JSON success is a single versioned
object with `ok`, `schema_version` `1`, `day_file`, `relative_day_file`,
`count`, `warnings`, and an ordered `pomodoros` array. Each Pomodoro has `ref`
(`<line>:<digest>`), `line`, `state` (`open` or `completed`), `status_symbol`,
nullable `name`, `slug`, `selectable`, nullable `time_range`, `placeholder`,
`is_current`, `child_count`, and `task_link_count`. `task_link_count` is the size of the numbered Task Link lineup that a plain `x` close indexes, the same count as `pomodoro_close.task_links`, present as an integer only on the `is_current` entry and `null` elsewhere. Editors use it to know whether close task numbers can be multi-digit. The ref is stale-safe: a later write command
can resolve exact line plus digest first, then a unique shifted digest match.
Named Pomodoros use the same slug rules as task sections — ASCII-lowercase,
whitespace collapsed to `-`, whole-slug matching before the first slug-prefix
match — with `+` also allowed in the selector. Entries whose names produce
untypeable slugs stay in the list with `selectable: false`.

With `-t/--tasks`, `capture-pomodoros` resolves each listed entry's Task Links
into agenda views. The top level adds `date` (`YYYY-MM-DD`, from the day file's
name or today) and `completed_summary` (`count` of completed ledger entries and
their summed `minutes`: each entry's recorded `[t:: Nm]` or stopwatch duration
when present, else its time-range span). Each entry adds a derived `homes`
object (the shared R9 shape from [Pomodoro homes](pomodoro-homes.md): `state`,
`kind`, `label`, nullable `chip` and `hue`, and `parts`). Open entries are a
forecast; completed entries are an attribution with `minutes` on each part.
Human output appends a dim chip with its forecast `○` or attributed `●` dot
after the entry name when `chip` is set. The schema stays version `1`. Calls
without `--tasks` omit `homes`. Each entry also adds `role` (`current`, `next`,
`later`, `open`, or `completed`), naive `starts_at`/`ends_at` datetimes
(`"YYYY-MM-DDTHH:MM"` for timed entries, `null` otherwise; both keys are always
present with `--tasks`, and an end before the start rolls to the next day),
`retired_link_count`
(struck single-link lines), `notes` (session notes: sub-bullet lines that are
neither items nor retired links nor descendants of an item), and `items` (every
line the `x` recognizer accepts: any depth, plain, deferred `[[T]]#`, or
embedded, unstruck, outside fences). Each item carries its operator `index`, the
1-based `ledger_line` and entry-relative `ledger_depth`, the `marker`
(`plain`, `deferred`, or `embedded`), the verbatim `block_link`, the
`resolution` (`resolved`, `missing_note`, `ambiguous_note`, `missing_block`,
`duplicate_block`, `not_a_task`, or `unreadable`), the vault-relative
`relative_target`, the 1-based task `line`, the `block_id`, the clean title
`text`, `status_symbol`, `status_name`, `status_type` (the same uppercase labels
`capture-tasks` uses), the first 150 task block `lines` (`lines_truncated`
counts the rest), the link line's `ledger_notes`, and a human
`warning` for any non-`resolved` resolution. A display line has `text` (task
lines use the clean description, bullets the body after the list marker, other
lines the line without indentation), relative `depth`, `kind` (`bullet`,
`task`, `text`, `code`, or `log_marker`), `status_symbol` on task lines, and a
`work`/`schedule`/`rotten` `log` tag on managed log markers and their subtrees.

Numbering follows the session operators exactly: on the current entry `index`
is the `x` number, and on every other open entry it is the `=` lineup number
matched by ledger line (`null` where that rule numbers nothing, such as deferred
or nested links on a non-current entry). Later entries are numbered too, because
`=#NAME~K` drops by those same lineup numbers; `==#NAME~K` is retired. `--all --tasks`
is allowed; completed entries report `role: "completed"` with empty `notes`
and `items`. Resolution never fails the command: each distinct note is read and
scanned once per filter mode (embedded links resolve with the global filter
cleared), empty link targets resolve to the daily note, and one bad link
becomes that item's `resolution` and `warning` while a missing daily note or
missing `## Pomodoros` section keeps the successful empty list with a warning.
When the daily note has a `## Pomodoros` section, the top level also adds
`plan_budget` with `status`, `themes`, and `links`. Each meter contains the saved
open-ledger `count`, configured `cap`, and `over` flag (`count > cap`). It uses
the same `plan:` limits loaded by capture (defaults are 3 themes and 10 links),
and the counting rules in [Plan budgets](plan.md); it does not include capture
growth, destination, or warning details. Empty and completed-only ledgers include
real zero counts. A missing daily note or section omits the budget. If the plan
configuration is invalid or unreadable, the agenda remains available, omits
`plan_budget`, and adds one bounded `plan budget unavailable: ...` warning. A
missing config file uses the loader's defaults. Calls without `--tasks` neither
load plan configuration nor include the budget. The budget depends on both the
daily-note contents and plan configuration; the rest of the agenda is derived
from its existing note inputs.

`capture-task-sections` lists the ALL-CAPS direct-child section bullets of one
parent task in document order. Exactly one of `--block-id`/`-i` or
`--task-ref`/`-t` is required; the parent lookup and error messages match
`bob capture` (missing ID with a close-match suggestion, duplicate ID, non-task
block ID, stale or ambiguous ref). A resolved task with no sections returns a
successful empty list so a picker can skip the chooser. Human output uses cyan
titles and dim slugs plus child counts, with a `No task sections found.` empty
state. JSON success is a single versioned object with `ok`, `schema_version`
`1`, `route`, nullable `block_id` (null when the parent was resolved by
`--task-ref` and still has no ID), `ref`, `count`, and an ordered `sections`
array. Each section has `title`, `slug`, `line`, `child_count`, and `depth`
(always `1` for a direct child). JSON failure is `{"ok": false, "error": "..."}`.

## `bob capture-templates`

```text
bob capture-templates [list] [-f|--format human|json]
bob capture-templates show TEMPLATE [-f|--format human|json]
bob capture-templates save [-f|--format human|json] [TEXT]...
bob capture-templates delete TEMPLATE... [-f|--format human|json]
bob capture-templates clear [-f|--format human|json]
```

The group is a capture-protocol sibling of `bob capture`; its verbs must stay
under `capture-templates` because free-text `bob capture` accepts arbitrary
text. `list` is the default and is read-only. An older bob reports an
unrecognized subcommand for `bob capture-templates` and exits 2. `show`
writes an unstyled draft to stdout. `save` reads TEXT joined with spaces, or
stdin when TEXT is absent. `delete` resolves numeric selectors against the
locked list at write time. All commands support `-f|--format human|json`.
JSON errors use `{"ok":false,"error":"...","code":"..."}`.

`list -f json` is the app protocol: each entry includes the insertable `draft`,
the UTF-8 byte `cursor`, `field_count`, `line_count`, the first row's `route`,
and `spans` computed by the same parser as `capture-parse`. Clients render and
insert these values directly instead of parsing template syntax. Saving is
controlled by `capture.max_saved_templates`; a newer schema blocks writes but
still permits reads, and `clear` remains available as the explicit reset.

```json
{
  "ok": true,
  "schema_version": 1,
  "capacity": 36,
  "saving": true,
  "file": "/Users/bryan/.local/state/bob-cli/capture/templates.json",
  "templates": [
    {
      "index": 1,
      "id": "9c1f0e2a7b3d",
      "text": "Apply to {0}! @job^apply-{1}\n- **URL**: {2}",
      "draft": "@@template:\nApply to {0}! @job^apply-{1}\n- **URL**: {2}\n\n",
      "cursor": 57,
      "field_count": 3,
      "line_count": 2,
      "route": "job",
      "spans": [{ "start": 9, "end": 12, "kind": "template_placeholder", "field": 0 }],
      "first_saved_at": "2026-10-16T09:12:44-04:00",
      "last_used_at": "2026-10-16T09:40:02-04:00",
      "use_count": 3
    }
  ],
  "warnings": []
}
```

`show -f json` returns `{"ok":true,"template":...}`. `save -f json`
returns `saved: {id,status}` and the new entry; `status` is `new`, `updated`,
or `replaced`. A new explicit save starts with `use_count: 0`. Delete returns
the deleted ids and any unresolved selector strings in `missing`; clear
returns the number of entries removed. Error JSON includes `ok: false`,
`error`, and `code`: `saved_template_not_found` exits 1; `saved_template_not_template`,
`saved_templates_disabled`, existing `template_*` definition errors, and an
unknown `--flag` where the draft would begin (`saved_templates_unknown_flag`) exit
2; `saved_templates_newer_schema`, `saved_templates_busy`, and
`saved_templates_io` exit 1.

## `bob capture-task-id`

```bash
bob capture-task-id (--route NAME | --note-path PATH) --task-ref REF --block-id ID [-b|--bob-dir DIR] [-f|--format human|json] [-d|--dry-run] [-a|--allow-closed]
```

Assigns a user-authored Obsidian block ID to one open task in a Bob note.
This is the only write needed to turn a missing-ID `capture-complete --all-tasks`,
`task_link`, `task_parent`, `task_dependency`, `blocked_task`, or `task_complete` candidate into an identified task. The command validates
`--route` (or `--note-path`) and `--block-id` with Bob's shared grammar (`A-Z`, `a-z`, `0-9`, and `-` for the
ID; routes also allow `_`), resolves `--task-ref` with the same stale-safe
`<line>:<digest>` recovery as `bob capture --task-ref`, and then confirms the
task is still open and still lacks an ID. An ID already used anywhere in the
target note — including a non-task `^anchor` — is rejected. Success appends
` ^<id>` to the resolved physical task line, preserves that line's ending and
every unrelated byte, and replaces the note with one same-directory temporary
file rename. The write is observable only after that rename completes.
`--dry-run` returns the same success shape without writing.

`--route` names a routable root note. `--note-path` names the exact
vault-relative path of any task-bearing note, so nested, case-sensitive, and
quoted paths round-trip exactly without the lowercasing route parser; the two
flags are mutually exclusive. `--allow-closed` additionally permits
Done/Cancelled tasks — the dependency picker's completed history — without
reopening them, and never applies to non-task lines. The previous daily
snapshot stays read-only and fails with a guarded refusal, as do traversal,
absolute, ambiguous, and symlink-escaping paths.

JSON success is a single versioned object with `ok`, `schema_version` `1`,
`dry_run`, `route` (in `--route` mode) or `note_path` (in `--note-path`
mode), `relative_target`, the backend-formatted `dependency_replacement`
(`&note:id`, quoted when the locator needs it), the backend-formatted
`blocked_task_replacement` (`*note:id`, quoted when the locator needs it),
the backend-formatted
`complete_replacement` (`!note:id`, quoted when the locator needs it, in
both modes) for splicing the new ID into the Complete picker, the
backend-formatted `relation_to_replacement` (`~note:id`) and
`relation_from_replacement` (`~~note:id`), quoted when the locator needs
it, for splicing the new ID into the relation pickers, the canonical
`block_id`,
the updated one-based `line`, the updated `ref`, and a `task` object with
the same picker metadata as `capture-tasks` after the assignment. In `--route`
mode it also returns `parent_replacement` (`@route+block-id`) for resuming a
parent-task picker; this field is omitted in `--note-path` mode. JSON
failure is `{"ok": false, "error": "..."}` and is write-free, as are stale,
ambiguous, terminal, already-identified, duplicate, missing, unreadable-note,
and read-only-snapshot errors.

## `bob capture-pomodoro-name`

```bash
bob capture-pomodoro-name --pomodoro-ref REF --name NAME [-b|--bob-dir DIR] [-f|--format human|json] [-d|--dry-run]
```

Assigns a canonical ALL-CAPS name to one open, unnamed Pomodoro in today's
daily note. This is the only write needed to turn a nameable Pomodoro picker
candidate into a selectable named Pomodoro. The command canonicalizes `--name`
by trimming, collapsing internal whitespace to a single space, and
ASCII-uppercasing, then requires the task-section title grammar plus `+`
(`A-Z`, `0-9`, spaces, and `& ' ( ) + , . / -`, starting with a letter or
digit). `+` is allowed in the rest of a Pomodoro name, not as the first
character.
Uppercasing is deliberate: the vault's named-Pomodoro convention is ALL-CAPS,
and case cannot affect the selector slug.

The daily note is selected exactly as `bob capture` selects it: `BOB_DAY_FILE`
when set and nonempty, otherwise `<bob-dir>/YYYY/YYYYMMDD.md` from `BOB_NOW` or
the local date. `--pomodoro-ref` uses the same stale-safe `<line>:<digest>`
recovery as `bob capture-pomodoros`. The selected entry must still be open. An
entry that already has a selectable name is refused so callers type `#<slug>`
instead of renaming. A named-but-untypeable entry is the exception: naming it
is the repair, and the command replaces the existing em-dash tail rather than
appending a second one.

Success appends ` — NAME` to the resolved physical line after trimming that
line's trailing spaces, preserves that line's ending and every unrelated byte,
and replaces the note with one same-directory temporary file rename. The write
is observable only after that rename completes. The command re-scans the
written contents and refuses to report success unless the entry now parses with
the expected name and slug. `--dry-run` returns the same success shape without
writing.

JSON success is a single versioned object with `ok`, `schema_version` `1`,
`dry_run`, `day_file`, `relative_day_file`, the canonical `name`, the `slug` to
type, the updated one-based `line`, the updated `ref`, a `pomodoro` object
with the same picker metadata as `capture-pomodoros` after the assignment,
and `warnings` when a `themes:` vocabulary is configured and the name is
outside it (`theme_not_configured`, omitted when empty). Human output
prints those warnings after the success lines. `--dry-run` returns the same
success shape without writing.
JSON failure is `{"ok": false, "error": "..."}` and is write-free, as are
stale, ambiguous, completed, already-named, missing-note, missing-section, and
unreadable-note errors.

A `$<N>` capture writes `[?]` when it rolls a scheduled date; unscheduled task
captures remain `[ ]`, and unscheduled Pomodoro-linked tasks remain `[*]`.
Project notes and their `^prj` lifecycle tasks are covered in
[projects.md](projects.md). The command index lives in the
[CLI reference](reference/cli/index.md) and environment variables in
[configuration](reference/configuration.md).
