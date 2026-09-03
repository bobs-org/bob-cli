# Capture

`bob capture` writes tasks and bullets into the Bob vault without opening
desktop Obsidian. Companion commands parse in-progress drafts, complete markers
and wikilinks, list routes and tasks, assign block IDs, and name Pomodoros.
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
  - [Authored sub-bullets](#authored-sub-bullets)
  - [Clipboard](#clipboard)
  - [Task with a requested block ID](#task-with-a-requested-block-id)
  - [Pomodoro-linked tasks](#pomodoro-linked-tasks)
  - [Sub-bullets under existing tasks](#sub-bullets-under-existing-tasks)
  - [Pomodoro notes](#pomodoro-notes)
  - [Section bullets](#section-bullets)
  - [Command-line options](#command-line-options)
  - [Input, stdin, and JSON output](#input-stdin-and-json-output)
  - [Interactive editor markers](#interactive-editor-markers)
- [`bob capture-parse`](#bob-capture-parse)
- [`bob capture-rewrite`](#bob-capture-rewrite)
- [`bob capture-complete`](#bob-capture-complete)
- [Discovery commands](#discovery-commands)
- [`bob capture-task-id`](#bob-capture-task-id)
- [`bob capture-pomodoro-name`](#bob-capture-pomodoro-name)

## Grammar at a glance

One capture item is a parent line plus optional authored child bullets. Blank
physical lines split a draft into multiple items; each item is planned before
anything is written, and any failure rolls the whole batch back.

| Marker | Meaning |
| --- | --- |
| `@@route` | Shared task destination, anywhere in the draft, for otherwise-unrouted items |
| `@@route+block-id` | Shared parent-task destination, anywhere in the draft, for otherwise-unrouted items |
| `@route` | Write a task to `<route>.md` (default route is `mac_inbox`) |
| `@route#Section` | Write an ordinary bullet into a matching non-`Tasks` heading |
| `@route#` | Write an ordinary bullet into any non-`Tasks` heading |
| `@route^block-id` | Ordinary open task with a user-authored block ID |
| `@route:block-id` | Next-status (`[*]`) task plus a Pomodoro task link; scheduled tasks start Blocked (`[?]`) |
| `@route:block-id#pomodoro` | Same, linked under a matching named open Pomodoro or a new named future Pomodoro |
| `@route+block-id` | Ordinary child bullet under an existing task |
| `@route+block-id#section` | Child bullet under an ALL-CAPS section of that task |
| trailing bare `#` | Plain-text note on a Pomodoro (not a routed task) |
| `s:<N>` | `[scheduled::]` N days from today; checkbox-bearing captures start Blocked (`[?]`) |
| `p:<N>` | Write priority level N and roll a scheduled date in that level's window |
| `%`, `%N`, `%header` | Capture clipboard content as child bullets |

`#` is not one marker. Read it by what it is attached to:

| You typed | Meaning |
| --- | --- |
| `remembered the timeout #` | Pomodoro note |
| `@notes#Ideas` | Bullet under a heading in `notes.md` |
| `@notes#` | Bullet under any non-`Tasks` heading in `notes.md` |
| `@cash+id#requirements` | Child under that task's `REQUIREMENTS` section |
| `@sase:deep-fix#bugs` | Pomodoro-linked task under open `BUGS`, creating future `BUGS` when needed |

A `#` in the middle of the body stays ordinary text. `@route::id` is retired;
use `@route^id` for an ordinary task with a block ID.

Leading `@route text` is accepted only on an item's first physical line. Later
lines in the same item take trailing markers only. `@@...` has no such
restriction: a declaration token may appear on any parent or authored child
line. Terminal `s:<N>`, `p:<N>`, `%...`, and `@...` markers configure the whole
item no matter which of its lines they appear on.

## `bob capture`

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures one task, ordinary Markdown bullet, or task sub-bullet into the Bob vault without
requiring desktop Obsidian to be open. `TEXT` is one or more physical lines: the
first nonblank line is the captured parent, and whitespace within each line is
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
the `Tasks` heading when the section has no tasks yet. Files without a `Tasks`
section keep the older fallback of inserting after the last top-level `#task`
block and its indented continuation lines, or appending at EOF.

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

Unsupported forms such as `@@foo#Ideas`, `@@foo^id`, and `@@foo:id` are errors
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
   `@bar#...`, `@bar^...`, `@bar:...`, and a trailing bare `#`.
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

Append a lowercase `p:<N>` token to write a priority level, where `N` selects
the Nth level in the bullet-property config file. Bob looks for that file at
`BOB_CONFIG_FILE`, then `$XDG_CONFIG_HOME/bob/config.yml`, then
`~/.config/bob/config.yml` — the same file the Obsidian picker reads. A missing
or unreadable file is an error; `p:<N>` has no built-in default levels. The
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
implicitly P0 (do it now, no roll), so there is no `p:0`. An explicit `s:<N>`
wins the scheduled date; `p:<N>` still writes the priority. A rolled `p:<N>`
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

A `p:<N> s:<N>` capture writes no entry, since `s:<N>` wins the scheduled date
and no roll happened. An out-of-range `p:<N>` fails with a usage error naming
the configured levels instead of staying literal. Any resolved scheduled
property makes a checkbox-bearing capture start Blocked (`[?]`); `bob
task-status-hooks` still reconciles tasks whose schedules are edited later.

### Multi-item capture

One or more blank or whitespace-only physical lines split `TEXT` into ordered
capture items. Leading, trailing, and repeated separator runs are ignored, so
a draft with at least one nonempty item is valid even if it starts or ends
with blank rows. Each item uses the normal capture grammar independently:
the first nonblank line is that item's parent, later contiguous authored
bullet rows belong only to that item, and terminal `s:<N>`, `p:<N>`,
`%...`, and `@...` markers configure only that item.

Bob plans the whole batch against in-memory note and daily-ledger snapshots
before writing anything. Later items see earlier planned edits to the same
target, so order, insertion points, duplicate block-ID checks, sub-bullet
lookups, and Pomodoro links match a successful sequential capture. If any
item fails to parse, read the clipboard, validate, stage, or replace, Bob
leaves notes, ledgers, and newly-created clipboard files at their original
state. `--dry-run` uses the same planner with the commit step disabled.

Single-item success JSON keeps the legacy shape. A multi-item success keeps
the first result in the legacy top-level fields and adds an ordered
`captures` array containing every per-item result. When a `@@` declaration is
present, success JSON also adds an optional top-level `global_destination`
object with `mode`, `route`, and optional `block_id`. The declaration never
creates an extra result. Human output numbers batch items as `1/N`, `2/N`,
and so on, and prints a compact `global  foo.md` (or `global  foo.md · under
^a-id`) summary when a declaration was used.

### Authored sub-bullets

`TEXT` may carry authored bullets beneath the parent line:

```text
Prepare the launch review
- Confirm the rollout owner
  - Send the owner the final date
- Attach the final checklist @work p:1
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
terminal `s:<N>`, `p:<N>`, `%...`, and `@route`/`@route#`/`@route^block-id`/
`@route:block-id`/`@route+block-id` marker is an item-wide directive no matter
which physical line in that item it appears on -- as shown above, `@work` and
`p:1` on the last child still route and prioritize that item -- and is
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
  `bob capture research links %3` captures three values. `%1` is equivalent to
  `%`, leading zeroes are accepted, and `%0` stays literal.
- `%<nonnumeric header>` captures the live clipboard once under an explicit
  header. Headers accept letters, digits, `_`, and `-`, render in uppercase,
  and replace underscores with spaces; for example, `%build_log` renders
  `**BUILD LOG:**`.

The marker composes with `s:<N>`, `p:<N>`, ordinary routes, bullet routes,
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

### Pomodoro-linked tasks

Use a leading or trailing `@<route>:<block-id>` marker to create a
Pomodoro-linked next task. For example,
`bob capture '@dev:foobar' 'Some foobar task.'` writes:

```markdown
- [*] #task Some foobar task. [created::2026-07-10] ^foobar
```

It also adds `[[dev#^foobar]]` as a child bullet of an eligible open Pomodoro
in today's daily note. The route is lower-cased; route and block-ID characters
are limited to letters, digits, `_`, and `-`. Scheduled offsets work in either
terminal order; a scheduled Pomodoro-linked task starts `[?]`, and the block ID
remains the final task token after any `[scheduled::YYYY-MM-DD]` property.

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
resolves even when the ledger has more than one open timed entry.

When the selector has no matching open entry, capture treats the authored
component as the name of a new future Pomodoro. The visible name uses the same
canonicalization as `bob capture-pomodoro-name`: whitespace is collapsed,
ASCII letters are uppercased, and allowed punctuation is preserved, so
`#after-tui-fix` creates `AFTER-TUI-FIX`. The new entry renders as
`- [ ] () — NAME` followed by the captured task link as its child. It is
inserted after the current Pomodoro's complete block when there is one,
otherwise after the last completed Pomodoro's complete block, otherwise before
the first Pomodoro in the section. Completed-only matches create a new future
Pomodoro with the same canonical name; completed history is not modified.
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

### Sub-bullets under existing tasks

Use a leading or trailing `@<route>+<block-id>` marker to capture an ordinary
child bullet beneath an existing task, without creating a note or changing the
parent task. For example,
`bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'` writes:

```markdown
- [*] #task Finish Google Exit Packet! [created::2026-07-31] ^goog-exit
  - Called Morgan Stanley today.
```

When the selected task already has a direct-child Schedule Log or Work Log, the
complete new child — including any authored children, clipboard children, or a
`p:<N>`-generated Schedule Log nested under that child — is inserted immediately
before the earliest of those managed logs. A Schedule Log is the
`🗓️ **SCHEDULE LOG**` child that records schedule changes; a Work Log is the
`🛠️ **WORK LOG**` child that records work summaries. Nested or lookalike log
markers do not move the insertion point. Tasks with neither managed log still
append at the end of the task block.

The marker composes with terminal `s:<N>`, `p:<N>`, and clipboard markers in
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

### Pomodoro notes

Append a bare trailing `#` marker to capture the item as a plain-text
sub-bullet on a Pomodoro instead of a task. For example,
`bob capture remembered to bump the timeout #` writes:

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
rejected alongside `s:<N>`, `p:<N>`, any `@route` token, and `--route`, since
a plain Pomodoro bullet has no field for a schedule, priority, or routed
destination:

| Marker                                                         | With `#` |
| ---------------------------------------------------------------| -------- |
| `%`, `%<N>`, `%<header>`, `--clip[=HEADER]`                     | allowed  |
| `s:<N>`                                                         | rejected |
| `p:<N>`                                                         | rejected |
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
insensitively), or any non-`Tasks` section when the marker is a bare `#`. A
matching non-H1 section is preferred; a matching H1 heading is used only when no
non-H1 heading matches. If no heading matches, the bullet goes into the
pre-heading (zeroth) section. Within the chosen section the bullet is inserted
after the last existing top-level bullet, otherwise just below the heading (or
after any YAML frontmatter for the zeroth section). The suffixed route token may
lead or trail the body, so `@notes#Ideas jot idea` and `jot idea @notes#Ideas`
both capture into `notes.md`. A standalone terminal `#<section-prefix>`
marker not appended to an `@route` token, such as `note #Ideas @foo`, is
still not accepted and fails with a usage error; a standalone bare `#`, such
as `note @foo #`, is instead the Pomodoro-note marker described above and
still fails, since it conflicts with the `@route` token on the same item.

A `--route` target keeps `@tokens` literal. Add `--section TITLE` with
`--route` to force bullet mode and place the bullet in a non-`Tasks` heading
whose title matches `TITLE` exactly, compared case insensitively. If no heading
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
- `-f, --format human|json`: human confirmation or stable JSON for callers
- `-n, --no-clip`: keep trailing `%...` clipboard markers literal
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
`bob capture -- "$(printf 'parent\n- child\n')"`. Embedded newlines inside a
single `TEXT` argument stay intact; multiple `TEXT` arguments are still
joined with single spaces, never newlines. Editor clients such as Bob Mac Capture should
call `bob capture --format json -- <text>` and parse the JSON object, whose
stable fields include `ok`, `dry_run`, `routed`, `route`, `route_label`,
`relative_target`, `target`, `text`, `task_line`, `kind`, `created`, and
`placement`. The `kind` field is `"task"`, `"bullet"`, `"pomodoro_task"`,
`"pomodoro_note"`, or `"sub_bullet"`, and `task_line` holds the rendered line
for any kind. On JSON-mode failures, stdout is still a
single object with `ok: false` and an `error` string.

A capture with authored sub-bullets additionally includes a `sub_bullets`
array of the exact rendered child lines, including their target-selected
indentation, in source order; it is omitted entirely for an ordinary
capture with no authored children. Human output prints those lines directly
beneath `task_line`, before any clipboard children and schedule log.

A `p:<N>` capture additionally includes `priority` (the written value, such as
`"high"`) and `priority_label` (the configured label, such as `"P1"`); a
capture without `p:<N>` omits both fields.

A `p:<N>` capture that actually rolled the scheduled date additionally
includes a `schedule_log` object: `reason` (the `🎲 …` text) and `lines` (the
exact rendered `🗓️ **SCHEDULE LOG**` marker and entry lines, in note order).
The schema is unchanged; the reason text records the exact selected day count
in bold and the configured range in parentheses. `schedule_log` is omitted
when `p:<N>` was not given, or when `s:<N>` won the scheduled date and no roll
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
`block_id`, `day_file`, `block_link`, and `pomodoro_link_placement`.

Sub-bullet results additionally include `parent_line`, `parent_text`,
`parent_status_symbol`, and `parent_status_name`. A capture that targeted a
task section also includes `parent_section` (the matched original title);
plain `@route+block-id` captures omit it. They reuse `block_id` for the
parent's ID, omitting it when a task-ref selected a parent without one.

Pomodoro-note results use kind `"pomodoro_note"` with `routed: false`, `route:
null`, and `target`/`relative_target` set to the daily note. They additionally
include `day_file`, `parent_line`, and `parent_text` describing the selected
Pomodoro's ledger line and text, but omit `block_id`, `block_link`,
`pomodoro_link_placement`, `parent_status_symbol`, and `parent_status_name`,
since the ledger checkbox is not an Obsidian task. Human output prints an
`under <parent_text>` line without a status marker, then the rendered
`- <text>` bullet.

### Interactive editor markers

Bob Mac Capture (Control-Shift-Command-I by default) also supports incomplete
interactive markers. Use `<task> @:` to choose an area or project and then enter
a block ID, `<task> @route:` to prompt only for the block ID, or
`<task> @:block-id` to prompt only for the destination. A complete
`<task> @route:block-id` request captures immediately. The panel validates
each supplied or prompted component, emits only the canonical colon marker,
and retains staged values when validation or capture fails. Existing `@`,
`@#`, and `@route#` picker flows are unchanged.

Supported terminal `%`, `%N`, `%header`, `s:<N>`, and `p:<N>` markers may
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

## `bob capture-parse`

```bash
bob capture-parse [-f|--format human|json] [--] [TEXT]...
```

Reports the authoritative capture grammar's reading of `TEXT` so an editor can
highlight capture syntax and Obsidian wikilinks while the user is still typing.
It shares one parser with `bob capture`: the same tokenizer, the same
terminal-marker extraction, and the same `@token` classification, so the two
commands can never disagree about a complete capture. Wikilink highlighting is
syntax-only and additive; it does not change capture routing or diagnostics.

The command is purely lexical and completely read-only. It never opens the
vault, never reads the clipboard, never touches the filesystem, and takes no
`--bob-dir`; running it with a nonexistent `BOB_DIR` and a `%...` clipboard
marker still succeeds. If `TEXT` is omitted and stdin is piped, it reads the
complete piped stdin stream, like `bob capture`. Only a missing `TEXT` or a
bad flag is an error (exit 2); every other input succeeds.

`TEXT` accepts the same batch draft `bob capture` does: one or more blank or
whitespace-only physical lines separate capture items, and each item keeps the
existing parent-plus-authored-bullets grammar. Within an item, the first
physical line is the parent, later column-zero `-`/`*`/`+` lines become
first-level authored children, and later lines prefixed by exactly two ASCII
spaces become nested authored children. Separator rows themselves have no
marker completion or highlighting. Incomplete
interactive markers are valid input rather than errors, so `@`, `@#`,
`@#Ideas`, `@route#`, `@^`, `@route^`, `@+`, `@route+`, `@route+id#`,
`@:`, `@route:`, `@route:id#`, `@route:#name`,
and the legacy `@!` aliases all parse on any line. The retired
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
the normalized capture body after terminal `s:<N>`, `p:<N>`, and `%...` markers
and the recognized `@...` token are removed, matching what `bob capture` would
write for any input it accepts. `mode` is `task`, `bullet`, `pomodoro_task`, `pomodoro_note`,
`sub_bullet`, or `incomplete`, describing whichever line resolved a marker
first -- the parent's leading or trailing form, or else the first child line
with a trailing marker. A bare trailing `#` reports `pomodoro_note` with
`route`, `section`, and `block_id` all `null` and an empty `needs` list. Combining
that marker with `@route`, `s:<N>`, or `p:<N>` on the same item still reports
mode `pomodoro_note` plus a `pomodoro_note_conflict` diagnostic; `bob capture`
rejects the same input. `route`, `section`, and `block_id` are the
resolved components, or `null`; `block_id` carries the ID-only task, Pomodoro,
or sub-bullet ID, whichever applies. For a Pomodoro marker, `section` carries
the Pomodoro name when one was typed — the same "whichever applies" reuse
`block_id` already has, and `mode` disambiguates. `needs` lists what a picker
still has to supply, in the
order `route`, `section`, `block_id`, `pomodoro_id`, `pomodoro_name`, `task`, `task_section`; it is an independent
completion hint, so the executable `@route#` bullet reports mode `bullet` and
needs `["section"]`, while `@route+id#` reports mode `incomplete` and needs
`["task_section"]` and `@route:id#` reports mode `incomplete` and needs
`["pomodoro_name"]`. A complete `@route+id#sec` sub-bullet populates `route`,
`block_id`, and `section` together. A complete `@route:id#name` Pomodoro marker
populates `route`, `block_id`, and `section` the same way.

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
This remains schema version 1.

For a multi-item draft, `items` is an ordered optional array, omitted for a
single item. Each entry has a one-based `index`, a `range` with global UTF-8
`start`/`end` offsets into `input`, `line_start`/`line_end` physical line
numbers, the item's `body`, `mode`, `route`, `section`, `block_id`, `needs`,
and optional `sub_bullets`/`sub_bullet_depths`. Real item indices and ranges
exclude declaration-only `@@` lines but still index the original draft. Top-level and
per-item `route`, `mode`, and `block_id` are the item's effective destination
after inheritance. The legacy top-level fields continue to describe the first
item so older clients retain a useful preview.

When the draft has a `@@` declaration, `global_destination` is an optional
object with the declaration `range`, one-based physical `line`, effective
`mode`, `route`, `block_id`, and `needs`. It is omitted when no declaration is
present, so schema version 1 stays additive.

`spans` are UTF-8 byte offsets into `input`, half-open `[start, end)`, ordered,
non-overlapping, and always on a character boundary. Each `kind` is one of
`route`, `section`, `task_block_id_route`, `task_block_id`,
`pomodoro_route`, `pomodoro_block_id`, `pomodoro_name`, `pomodoro_note`, `sub_bullet_route`,
`sub_bullet_block_id`, `sub_bullet_section`, `global_route`,
`global_sub_bullet_route`, `global_sub_bullet_block_id`, `schedule`, `priority`, `clipboard`,
`interactive_placeholder`, `wikilink_delimiter`, `wikilink_target`,
`wikilink_heading`, `wikilink_block_id`, or `wikilink_alias`. A placeholder
marks the part of a marker the user has not filled in yet: the trailing `+` in
`@cash+` or `@@cash+`, the trailing `#` in `@cash+id#`, or the whole `@+` /
`@@` when the route is still empty too. Wikilink spans
cover syntax only; unresolved note targets are not errors.

Each entry in `diagnostics` has `severity` (`error`, `warning`, or `info`), a
stable snake_case `code`, a `message` reusing `bob capture`'s exact wording,
and a nullable `range` given as a two-element `[start, end]` byte array.
Today's codes are `invalid_task_block_id_route`, `invalid_task_block_id`,
`retired_task_block_id_marker`, `invalid_sub_bullet_route`,
`invalid_sub_bullet_block_id`, `invalid_sub_bullet_section`,
`invalid_pomodoro_route`, `invalid_pomodoro_block_id`, `invalid_pomodoro_name`, `legacy_bullet_marker`,
`pomodoro_note_conflict` (a trailing bare `#` on the same item as `@route`,
`s:<N>`, or `p:<N>`),
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
prints the same information without color escapes when piped, plus a
`Sub-bullets` section listing `sub_bullets` with indentation from
`sub_bullet_depths` when it is nonempty. On a missing `TEXT`, JSON mode prints
a single `{"ok": false, "error": "..."}` object on stdout and keeps stderr
clean.

## `bob capture-rewrite`

```bash
bob capture-rewrite [-c|--cursor N] [-f|--format human|json] [--] [TEXT]...
```

Applies the capture grammar's automatic draft rewrites -- today, the bare
`@@` absorption rule -- and reports the resulting edits, cursor, and a human
summary. Like `bob capture-parse` it is purely lexical and completely
read-only: it never opens the vault, never reads the clipboard, never
touches the filesystem, and takes no `--bob-dir`. If `TEXT` is omitted and
stdin is piped, it reads the complete piped stdin stream. Only a missing
`TEXT` or a bad flag is an error (exit 2); every other input succeeds, with
`changed: false` when nothing needed to change.

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
`@route:block-id`, or a trailing bare `#` -- is left untouched; the result reports `changed: false` plus a
`notices` entry naming the marker and why, e.g.
`@@ cannot take a section: leave @notes#Ideas on this item, or delete it and declare @@notes`.
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

`rule` is `absorb_local_marker` or `absorb_declaration`, omitted when nothing
changed. `cursor` is present only when `--cursor` was supplied, mapped
through every edit so it lands just past the rewritten `@@<payload>` token.
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
prefixes. `--cursor`/`-c` is required and must be a UTF-8 byte offset on a
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
scan as `bob capture-targets`. Section completion covers `@route#prefix`,
backed by the same scan as `bob capture-sections`. Task-section completion
covers `@route+id#prefix` and a bare `@route+id#`, backed by the same scanner
as `bob capture-task-sections`; the candidate `replacement` is the section
slug (`future-work` for `FUTURE WORK`). Pomodoro-name completion covers
`@route:id#prefix`, a bare `@route:id#`, and `@route:#prefix`; only the route
must already resolve because the Pomodoro list does not depend on the block
ID. It is backed by the same scan as `bob capture-pomodoros`, offers only open
entries, and returns Pomodoros in picker order: named rows first, then
nameable rows. Named rows rank by slug prefix, then slug substring, and open
entries with the same slug collapse to the first row with `match_count`
reporting how many open Pomodoros share it. Nameable rows represent unnamed or
named-but-untypeable entries, set `requires_name: true`, use an empty
`replacement`, and are never filtered out by the query. Updated clients must
prompt for a name rather than inserting that empty replacement. When the query
is a nonempty valid name that would not select an open exact or prefix match,
and today's ledger can uniquely place a new future entry, a create action is
inserted before substring-only named suggestions and before nameable rows:
`creates_pomodoro: true`, canonical selector `replacement`, canonical visible
`name`, and no `ref`. Accepting that row only canonicalizes the marker; the
later `bob capture` transaction creates the named placeholder. Exact or prefix
open-name matches stay first and do not receive a create row. Empty queries
stay the existing discovery list. A missing daily note, a missing Pomodoros
section, and multiple open timed Pomodoros stay write-free warnings without a
create row. Pomodoro block-ID
completion covers `@route:prefix` and parent-task completion covers
`@route+prefix`; both are backed by the same open-task scan as
`bob capture-tasks`. By default both contexts only offer tasks that already
carry a block ID so older callers stay compatible. Pass `-a`/`--all-tasks` to
include open tasks that still need an ID, but only in the `task` / `@route+`
context. Pomodoro `@route:` completion stays identified-only even when
`--all-tasks` is set. Missing-ID discovery is therefore opt-in and
plus-context-only.
The right-hand side of `@route^block-id` is a new user-authored ID, so it has no
completion context and returns an empty successful result while the caret is
inside it. An empty block-ID component (`@route+#`) returns a successful empty
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
target, current-destination, or `[[^^...]]` vault-wide scope. The note index is
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
`context` is `route`, `section`, `pomodoro_block_id`, `pomodoro_name`, `task`,
`task_section`, `wikilink_note`, `wikilink_heading`, `wikilink_block`, or `null` when no completion field is
active. Each candidate's `replacement` is the exact text to insert; wikilink
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
rows use the slug as `replacement`, while nameable rows use an empty
replacement that clients must not insert. A create row omits `ref` and `line`,
sets `creates_pomodoro: true`, and uses the canonical selector as
`replacement`. Older clients may treat that row as an ordinary replacement. A task candidate
(`pomodoro_block_id` or `task` context) has `ref`, nullable `block_id`,
`route`, `requires_block_id`, `status_symbol`, `status_name`, `status_type`,
`text`, nullable `section`, `depth`, and `child_count`. Identified tasks keep
their normal block-ID `replacement`. Missing-ID tasks, which appear only when
`--all-tasks` is set in the `task` context, have `block_id: null`,
`requires_block_id: true`, and an empty placeholder `replacement` that the
updated client must never insert. A wikilink note candidate has `path`, `name`, optional `alias`,
and `match_kind`; heading and block candidates add `heading`/`level` or
`block_id`/optional `preview` metadata. Link-index warnings, when present, are
reported in a bounded top-level `warnings` array without logging draft text. A
missing note behind a resolved route or link target is not an error; it returns
an empty candidate list. Discovery failures never fall back to a default route
or an empty result silently; they return the same actionable error in human and
JSON forms as the underlying scan would.

## Discovery commands

```bash
bob capture-pomodoros [-a|--all] [-b|--bob-dir DIR] [-f|--format human|json]
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
`label`, `kind`, `is_default`, `status`, and `relative_path`. `--verbose`
reports top-level Markdown files omitted because their filename is not a valid
route; other omissions remain silent.

`capture-sections` lists each parsed ATX heading (H1-H6) except a heading
titled exactly `Tasks`, in document order. It ignores headings in YAML
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
`is_current`, and `child_count`. The ref is stale-safe: a later write command
can resolve exact line plus digest first, then a unique shifted digest match.
Named Pomodoros use the same slug rules as task sections — ASCII-lowercase,
whitespace collapsed to `-`, whole-slug matching before the first slug-prefix
match — with `+` also allowed in the selector. Entries whose names produce
untypeable slugs stay in the list with `selectable: false`.

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

## `bob capture-task-id`

```bash
bob capture-task-id --route NAME --task-ref REF --block-id ID [-b|--bob-dir DIR] [-f|--format human|json] [-d|--dry-run]
```

Assigns a user-authored Obsidian block ID to one open task in a routed note.
This is the only write needed to turn a missing-ID `capture-complete --all-tasks`
candidate into an identified task. The command validates `--route` and
`--block-id` with Bob's shared grammar (`A-Z`, `a-z`, `0-9`, and `-` for the
ID; routes also allow `_`), resolves `--task-ref` with the same stale-safe
`<line>:<digest>` recovery as `bob capture --task-ref`, and then confirms the
task is still open and still lacks an ID. An ID already used anywhere in the
routed note — including a non-task `^anchor` — is rejected. Success appends
` ^<id>` to the resolved physical task line, preserves that line's ending and
every unrelated byte, and replaces the note with one same-directory temporary
file rename. The write is observable only after that rename completes.
`--dry-run` returns the same success shape without writing.

JSON success is a single versioned object with `ok`, `schema_version` `1`,
`dry_run`, `route`, `relative_target`, the canonical `block_id`, the updated
one-based `line`, the updated `ref`, and a `task` object with the same picker
metadata as `capture-tasks` after the assignment. JSON failure is
`{"ok": false, "error": "..."}` and is write-free, as are stale, ambiguous,
terminal, already-identified, duplicate, missing, and unreadable-note errors.

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
type, the updated one-based `line`, the updated `ref`, and a `pomodoro` object
with the same picker metadata as `capture-pomodoros` after the assignment.
JSON failure is `{"ok": false, "error": "..."}` and is write-free, as are
stale, ambiguous, completed, already-named, missing-note, missing-section, and
unreadable-note errors.

A `p:<N>` capture writes `[?]` when it rolls a scheduled date; unscheduled task
captures remain `[ ]`, and unscheduled Pomodoro-linked tasks remain `[*]`.
Project notes and their `^prj` lifecycle tasks are covered in
[projects.md](projects.md). The command index and environment variables live in
the [root README](../README.md).
