---
title: Capture syntax
description: Index of bob capture tokens with syntax, examples, and links into the capture contract.
---

# Capture syntax

`bob capture` writes tasks, notebook notes, Record Notes, references, and Pomodoro changes.
See [Capture contract](../capture.md).

`@` chooses a destination; `#` names a section, drawer, or task type; `%` selects a
capture directive. In completion, standalone `#` and `%` open menus.

Examples use a synthetic vault (`BOB_NOW=2026-10-20 12:00:00`, `TZ=UTC`).
Put options before the draft; `--` ends option parsing.

## Token index

Incomplete **picker** input cannot execute.

| Token | Meaning | Heading |
| --- | --- | --- |
| Bare public URL | Queue a reference job under `mac_inbox` | [Bare public URL](#bare-public-url) |
| `URL @route` / `@route URL` | Queue the reference under that route | [`URL @route`](#url-at-route) |
| `URL %l` / `URL %l @route` | Queue with companion audio | [`URL %l`](#url-listen) |
| `-r route` | Force the URL (or task) route from the CLI | [`-r`](#cli-route) |
| `@@route` | Draft-wide destination for unrouted items | [`@@route`](#global-route) |
| `%t` | Expand one capture per template row | [`%t`](#template) |
| `%` | **Picker:** choose a capture directive | [Bare `%`](#percent-picker) |
| `#` | **Picker:** choose a task type | [Bare `#`](#hash-picker) |
| `@route` | Write a task to `<route>.md` | [`@route`](#at-route) |
| `@dir/` / `@dir/^stem` / `@dir/stem` | Record create, explicit stem, or existing-record update | [Record notes](#record-notes) |
| `@route#Section` / `@route#section^id` | Ordinary bullet under a matching heading | [`@route#Section`](#at-route-section) |
| Notebook `@route`, `@route#section`, `@route^id`, `@route+id` | Append a note, section note, note with an ID, or child note | [Notebook capture](../notebooks.md#capture) |
| `@route#` | Ordinary bullet under any non-`Tasks` heading | [`@route#`](#at-route-any-section) |
| `@route^block-id` | New open task with a user-authored block ID | [`@route^block-id`](#at-route-caret) |
| `@route:block-id` | New Next task plus a Pomodoro Task Link | [`@route:block-id`](#at-route-colon) |
| `@route:block-id#pomodoro` | Same, under a named Pomodoro | [`@route:block-id#pomodoro`](#at-route-colon-named) |
| `@route:block-id=<X>` / `@route:block-id=name` | Same, and start that session (`@work:outline=bugs` links under `BUGS` and starts it) | [`@route:block-id=<X>`](#at-route-colon-start) |
| `@route:block-id` with no other text | Link an existing task into today's ledger | [Link an existing task](#link-existing) |
| `^route:block-id` | Active-task spelling of a link | [`^route:block-id`](#caret-active-task) |
| `:<query>` | **Picker:** choose any open task to link | [`:<query>`](#colon-picker) |
| `/<query>` | **Picker:** choose any open task to open | [`/<query>`](#slash-picker) |
| `+query` | **Picker:** choose a parent task | [`+query`](#plus-parent-picker) |
| lone `+` | Editor parent picker; Escape leaves `+5m` unchanged | [lone `+`](#lone-plus) |
| `&note:block-id` | Captured task depends on that prerequisite | [`&note:block-id`](#ampersand-prereq) |
| `&` / `&query` | **Picker:** choose a prerequisite | [`&`](#ampersand-picker) |
| `^route:block-id &note:id` | Add a prerequisite to an existing owner | [Add a prerequisite](#add-prereq-existing) |
| `*note:block-id` | Named open task depends on the captured task | [`*note:block-id`](#star-blocks) |
| `*query` | **Picker:** choose a blocked task | [`*query`](#star-picker) |
| `^route:block-id *note:id` | Existing owner blocks an existing task | [Block from an existing owner](#star-existing) |
| `~note:block-id` | This task links to that task, with an optional label | [`~note:block-id`](#tilde-link) |
| `~note:block-id\|\|label` | Same link; the label hides from suggestions once the capture succeeds (a one-off) | [`~note:block-id`](#tilde-link) |
| `~~note:block-id` | That task links to this task, with an optional label | [`~~note:block-id`](#tilde-tilde-link) |
| `^route:block-id ~note:id` | Relate an existing owner | [Relate an existing owner](#tilde-existing) |
| `!` / `!query` | **Picker:** choose a task to complete | [`!`](#bang-picker) |
| `!note:block-id` | Complete that existing open task | [`!note:block-id`](#bang-complete) |
| `+[N]` | Extend today's current timed Pomodoro | [`+[N]`](#plus-adjust) |
| `-[N]` | Shorten today's current timed Pomodoro | [`-[N]`](#minus-adjust) |
| `++[N]` | Shift the running session later | [`++[N]`](#plus-plus-shift) |
| `--[N]` | Shift the running session earlier | [`--[N]`](#minus-minus-shift) |
| `=` / `=<X>` | Start the next future Pomodoro, or restart | [`=`](#equals-start) |
| `=name` | Start a named Pomodoro (`=bugs`, `=3bugs`, `=bugs3`, `=fix2+`) | [`=name`](#equals-named) |
| `=/[base][#type…]` | Split types into separate planned Pomodoros | [`=/`](#equals-split) |
| `=*[source][#destination…]` | Merge the next planned Pomodoro or named destinations into one open session | [`=*`](#equals-merge) |
| `~<K>` | Drop those queued Task Links from the start | [`~<K>`](#tilde-drop) |
| `x` | Close today's running timed Pomodoro | [`x`](#x-close) |
| `[N]x` | Close at N blocks | [`[N]x`](#nx-close) |
| `x=E[s][!][=X]` | Record a past Pomodoro ending at the next five-minute boundary or segment | [Recording past Pomodoros](#recording-past-pomodoros) |
| `x!<M>` | Close, completing those Task Links | [`x!<M>`](#x-complete-links) |
| `x*<P>` | Close, parking those Task Links | [`x*<P>`](#x-park-links) |
| `x~<K>` | Close, dropping those Task Links | [`x~<K>`](#x-drop-links) |
| Work Log | Log work onto the close's worked tasks | [Work Log](#x-work-log) |
| `+2 x` / `x =` | Chain session operators on one line | [Operator chain](#operator-chain) |
| `@route:block-id=x` | Link that task, then close | [`@route:block-id=x`](#link-then-close) |
| `@route^block-id@` | Project note, or notebook with `%n` | [`@route^block-id@`](#project-note-seal) |
| `@route:block-id@` | Create a project note and offer its open tasks | [`@route:block-id@`](#project-note-offer) |
| `:id` | Project-note child: name, Next, and link | [`:id` child](#project-colon-child) |
| `^id` | Project-note child: name only | [`^id` child](#project-caret-child) |
| `@route+block-id` | Child bullet under an existing task | [`@route+block-id`](#plus-child) |
| `@route+id @route+id` | Target run | [Target run](#plus-child-target-run) |
| `@a+x @b+y > @c` / `@a+x @b+y > @c+id` / `@a+x > @c+id#drawer` / `@a+x > @c#section` | Move or merge tasks with `>` | [`>` transfer](#transfer) |
| `T < @a+x @b+y` | Merge into the new task | [`<` merge-in](#merge-in) |
| `@route+block-id#drawer` | Child inside a task drawer | [`@route+block-id#drawer`](#plus-child-section) |
| `- #drawer` / `- #Section` | Drawer, or notebook section under `@nb`/`%n` | [Drawer bullets](#drawer-bullets) |
| `@route+block-id` with no other text | Ensure Next and move its Task Link | [Ensure Next](#plus-ensure-next) |
| `@route+block-id!` | Toggle that task's Pomodoro Task Link | [`@route+block-id!`](#plus-toggle) |
| `%n` | Pomodoro note, or notebook with `@parent^id@` | [`%n`](#trailing-hash) |
| `s:<N>` | Schedule N days from today; checkbox tasks start Blocked | [`s:<N>`](#s-schedule) |
| `$` / `$<N>` | Write priority N and roll a scheduled date | [`$`](#dollar-priority) |
| `%c` / `%c<N>` | Capture clipboard content as child bullets | [`%c`](#percent-clipboard) |
| `#<type>` | Type a new task or retype its existing target | [`#<type>`](#hash-type) |

## Overloaded markers

| Marker | Context | Meaning |
| --- | --- | --- |
| `+` | `+[N]`, `++[N]`, `+query` | Resize, shift, or open the parent-task picker |
| `,` | Parent Task picker positions only | Alias `+`; after a complete target run, insert ` +` to open the next picker |
| `.` | Right after `@route`; after a space following text | Alias `^`, `@`, or `&`; spaced `;` aliases `*` |
| `+` | `@route+id`; typed-last owner; after `#name` | Child/Ensure Next, typed-last alias, or name suffix |
| `%n` | Item edge | Pomodoro note |
| `#` | `@route#Section`, edge `#type`, existing-task `#type` | Heading, task type, or retype |
| `#` | `@route+id#drawer`; `- #drawer`; `x=#"NAME"` | Drawer, drawer shortcut, or exact past-session name |
| `@` | `@route`, `@@route`, `URL @route`; after project owner ID | Note/draft/reference route or project seal (`:id@` offers tasks) |
| `@` | `x@N` | Close keep-list; `@` is retired as start or past timing |
| `%l` | URL | Request companion audio |
| `*` | `*note:id`, `x*`, `=*source` | Block task, park links, or merge sessions |
| `/` | `=/base#type` | Split a merged session |
| `*` | Line-initial bare `*` | Markdown bullet, not a blocked-task marker |
| `~` | `~note:id|label`, `~note:id||label`; `x~`, `=~` | Relation label or drop close/start links |
| `~` | `~/path`, `~5m`, mid-line `~x:y` | Not relation syntax |
| `~~` | `~~note:id`; `~~old~~` | Reverse relation; strikethrough |
| `!` | `!note:id`, `x!`, `x=E!` | Complete task/links or permit closed-edge repair |
| `%` | Bare `%`, `%c` / `%c<N>`, `%n` | Directive picker, clipboard, or Pomodoro note |

Contract: [Grammar at a glance](../capture.md#grammar-at-a-glance).

## Incomplete input versus executable syntax

A picker token is draft state. `capture-parse` reports its needs;
`bob capture` refuses it until selection produces
`@route:id`, `!note:id`, `&note:id`, `*note:id`,
`~note:id`, `~~note:id`, or
`@route+id`. Session tokens needing a number or name (`x~`, `=~`, `x@`,
`=#`) are incomplete too. A bare `=name` start is complete (`=bugs`); an empty `=#` needs a non-bare name (`pomodoro_name`).

A signed count with extra prose (`+2 more`, `=3 more`) is an error.
A sign or `=` followed by prose (`Plan +5`, `= foo`) stays task text. Capital `X` never closes; `X *job:cleanup` captures a
task starting with the word `x`.

## Retired and rejected forms

Bob teaches the replacement.

| Typed | What happens |
| --- | --- |
| `==`, `==<X>`, `==#name` | Retired restart/swap; write one `=` (`=`, `=3`, `=bugs`) |
| `=NAME@N`, `=<N>#BARE` | Retired named-start spellings; glue timing to a bare name (`=bugs3`, `=3bugs`) |
| `==x` | Error: close with `x` |
| `@route^id+` / `@route:id+` (with or without `#pomodoro`) | Retired project-note forms; write `@route:id@` to choose which tasks to link or `@route^id@` to link none |
| `@route^id#pomodoro@` / `@route:id#pomodoro@` | Error: the seal goes right after the ID (`@route:id@#pomodoro`) |
| `@route::id` | Retired ordinary block-ID form; write `@route^id` |

| `#now` / a free `#tag` at an item edge | Refused with `type_not_configured` unless the name is configured; a bare `@route` adds the heading hint |
| `t:fix` | Ordinary prose; `t:` is retired as a type marker |
| `x#bugs` | Error: `x` always closes the running session; write `x =bugs` to close then start |
| `x=3-1`, `x=-2`, `x=3-` | No past offsets; write `x=3~1` or drop the dash (`x=3`) |
| `x=NAME@N`, `x=<N>#BARE` | Retired past spellings; write `x=fix3` or `x=3fix` |
| `x=*3`, `x=1!2` | Put close modifiers after a second `=` (`x==*3`, `x==1!2`) |
| `x=0`, `x=build@0` | Need at least one block: `x=1`, `x=build1` |
| `x=/build`, `x=*build` | `x=` cannot split or merge a session |
| `x=fix!2` | Use `x=fix=!2`; to repair too, use `x=fix!=!2` |
| `x=3x` | Close-shaped name: write `x=3#x` |

Contract: [Retired close spellings](../capture.md#retired-close-spellings).

## Recording past Pomodoros

### `x=E[s][!][=X]` {#recording-past-pomodoros}

`x=E[s][!][=X]` records a past Pomodoro. `E` is empty (25 minutes), a
positive count, or a name. Counts can go before or after bare names
(`x=3fix`, `x=fix3`); names with `-`, a trailing digit or `+`, or close
shape use `#` (`x=#c++`). Records stack backward from the
next five-minute boundary unless a segment suffix (`~…`) places them;
`!` permits trimming an older closed edge. See the
[past Pomodoro contract](../capture.md#placing-it-on-todays-timeline).

## Routes and destinations

### `@route` {#at-route}

Write a `#task` into `<route>.md` at the vault root, or a plain note when
the destination is a [notebook](../notebooks.md#capture). Route names are
`A–Z a–z 0–9 _ -`, lower-cased; the default is `mac_inbox`.

```bash
bob capture 'Review the queue input'
```

```text { .no-copy title="Output" }
✓ captured  mac_inbox.md
  - [ ] #task Review the queue input [created::2026-10-20]
```

```bob-task
- [ ] #task Review the queue input [created::2026-10-20]
```

Contract: [Routing and insertion](../capture.md#routing-and-insertion).

## Record notes

### `@dir/` / `@dir/^stem` / `@dir/stem` {#record-notes}

Requires an album or config [record kind](../records.md#schema).

```text
Ramen Nagi @eat/
- status:: liked
- category:: Ramen
- location:: Clark
```

```bash
bob capture --dry-run -- "$(printf '%s\n' 'Ramen Nagi @eat/' '- status:: liked' '- category:: Ramen' '- location:: Clark')"
```

`@dir/` creates a record; `@dir/^stem` fixes its filename. `@dir/stem`
updates an existing record: supplied fields change and prose appends to Notes.
See [Capture](../capture.md#record-notes).

### `@@route` {#global-route}

A draft-wide destination for otherwise-unrouted items. It may sit anywhere.
A plain `@@route` also routes default (routeless) refs as source `global`.
Notebook `@@route` routes notes; `@@nb#sec`, `@@nb^id`, `@@nb+id` are invalid.

```bash
bob capture 'Buy milk @@work'
```

Contract: [Global destination declaration](../capture.md#global-destination-declaration).

### `@route#Section` / `@route#section^id` {#at-route-section}

Ordinary bullet under a matching non-`Tasks` heading; append `^id`
(`#"Title^lit"` keeps the caret).

```bash
bob capture 'Remember the timeout @notes#Ideas'
bob capture 'Try mergiraf @dev#ideas^mergiraf'
```

Contract: [Section bullets](../capture.md#section-bullets).

### `@route#` {#at-route-any-section}

Same, targeting any non-`Tasks` heading. Generated status-group headings
under `Tasks` are never bullet targets.

```bash
bob capture 'Remember the timeout @notes#'
```

```text { .no-copy title="Output" }
✓ captured  notes.md
  - Remember the timeout [created::2026-10-20]
```

Contract: [Section bullets](../capture.md#section-bullets).

### `-r` / `-r route` {#cli-route}

`--route NAME` forces `NAME.md` and keeps typed `@tokens` literal. On a bare
URL it queues the reference under that route with source `explicit`.
Notebook `--route` forces a note.

```bash
bob capture -r work -- 'https://example.com/article'
```

Contract: [Command-line options](../capture.md#command-line-options) and
[Saving links](../capture.md#saving-links-to-your-reading-queue).

#### Inbox task destinations

A capture that changes an existing inbox task's status reports its top-level
task block as a route candidate; a nested change routes that whole block.
Unanswered candidates stay in place. Repeatable
`-i N=note` or `--inbox-route KEY=note` moves a candidate after the capture action;
`-i N=` keeps one, and `-i keep` keeps every otherwise unanswered candidate.
Numbers and keys come from that draft's `inbox_route` preview.

For an existing Ready task `^triage` in `mac_inbox.md`:

```bash
bob capture --dry-run -f json -- '@mac_inbox:triage'
bob capture --dry-run -i 'mac_inbox.md#^triage=work' -- '@mac_inbox:triage'
```

Both commands only preview. The second plans the link, then the move to
`work.md` and link repairs; removing `--dry-run` from it commits the action
and move together. Do not commit the first command before answering. New
tasks and blocks with no status changes produce no candidates.
Destinations exclude inboxes; stale keys or refused moves abort the batch.

Contract: [Routing inbox tasks out](../capture.md#routing-inbox-tasks-out).

## URL routing and audio

### Bare public URL {#bare-public-url}

When the URL routing policy admits it, a bare public URL queues a background
reference job under `mac_inbox`. Confirmation says reading queue; the
job files the note later. `-R, --no-ref` keeps it a task.

```bash
bob capture --dry-run 'https://example.com/article'
```

```text { .no-copy title="Output" }
[dry-run] ok would queue  example.com/article → mac_inbox
  new to your library · reading task lands in mac_inbox.md · file it later
```

```bash
bob capture --dry-run --no-ref 'https://example.com/article'
```

```text { .no-copy title="Output" }
[dry-run] ok would capture  mac_inbox.md
  - [ ] #task https://example.com/article [created::2026-10-20]
```

Contract: [Saving links to your reading queue](../capture.md#saving-links-to-your-reading-queue).

### `URL @route` {#url-at-route}

Queue the reference under that route (aliases resolve; source `explicit`).
`@route URL` is the same.

```bash
bob capture --dry-run 'https://example.com/article @work'
```

```text { .no-copy title="Output" }
[dry-run] ok would queue  example.com/article → work
  new to your library · reading task lands in work.md · file it later
```

Contract: [Saving links to your reading queue](../capture.md#saving-links-to-your-reading-queue).

### `URL %l` {#url-listen}

Queue the reference with companion audio (`ref.listen` true). Parent
selection matches a bare URL: default, `@@route` global, or `-r`.
`URL %l @route` / `@route URL %l` sets an explicit parent.

```bash
bob capture --dry-run 'https://example.com/article %l @work'
```

```text { .no-copy title="Output" }
[dry-run] ok would queue  example.com/article → work · Listen
  new to your library · reading task lands in work.md · companion audio requested
```

Contract: [Saving links to your reading queue](../capture.md#saving-links-to-your-reading-queue).

## Templates

### `%t` {#template}

`%t` at either edge of any line of the draft's first item makes that item a
template. The `{N}` placeholders in it capture exactly like its expansion: one
item per nonblank row after the first blank line (`{0}`-only templates can pack
several values on one line). `-E, --expand` prints the expansion and exits
before vault access.

```bash
printf '%s\n' 'Call {0} @work %t' '' 'Alex' 'Sam' | bob capture -E
```

```text { .no-copy title="Output" }
Call Alex @work

Call Sam @work
```

A successful real `%t` capture shelves the template block, not the rows;
manage it with `bob capture-templates`. A `%t` anywhere but the first
item is refused (`template_directive_misplaced`).

Contract: [Bulk templates](../capture.md#bulk-templates).

## Block IDs and linked tasks

Editor `capture-rewrite` aliases include `@route.` / `@route;` → `^` / `:`,
whole-item `;` → `:`, and spaced `.` / `;` → route or dependency pickers.
A trailing `.` toggles complete `@route:id` ↔ `@route^id`, including sealed
project markers and task-list IDs. Direct CLI capture never applies these
[typing assists](../capture.md#bob-capture-rewrite).

### `@route^block-id` {#at-route-caret}

Create an ordinary open task with that block ID.

```bash
bob capture 'Write an outline @work^outline'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [ ] #task Write an outline [created::2026-10-20] ^outline
```

```bob-task
- [ ] #task Write an outline [created::2026-10-20] ^outline
```

Contract: [Task with a requested block ID](../capture.md#task-with-a-requested-block-id).

### `@route:block-id` {#at-route-colon}

Create a Next (`[*]`) task and a Pomodoro Task Link. A future `s:<N>`
(N ≥ 1) starts Blocked (`[?]`) and unlinked.

```bash
bob capture 'Write an outline @work:outline'
```

Contract: [Pomodoro-linked tasks](../capture.md#pomodoro-linked-tasks).

### `@route:block-id#pomodoro` {#at-route-colon-named}

Same, linked under a matching named open Pomodoro, or a new named future
Pomodoro.

```bash
bob capture 'Write an outline @work:outline#focus'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [*] #task Write an outline [created::2026-10-20] ^outline
  Linked under FOCUS
```

Contract: [Pomodoro-linked tasks](../capture.md#pomodoro-linked-tasks).

### `@route:block-id=<X>` / `@route:block-id=name` {#at-route-colon-start}

Start it: `=3` is 15m; `=+2` starts 10m later; `=` is 25m.
`=bugs` equals `#bugs=`.

Operator-only link-start bodies fail `invalid_pomodoro_link` on their range.
One `++[N]` teaches glued `+`; prose stays text.

```bash
bob capture '@work:outline=3'
```

```bash
bob capture '@work:outline=bugs'
```

| Suffix on `@work:outline` or `^work:outline` | Session and timing |
| --- | --- |
| `=focus3`, `=3focus` | FOCUS, 15m |
| `=focus-2` | 10 minutes earlier |
| `=+2`, `=+focus` | 10 minutes later |
| `=#2fa`, `=3#deep-work` | `#` escapes names that cannot be spelled bare |
| `=focus@3`, `=3#focus` | Retired; write `=focus3` or `=3focus` |

Link-and-start fills a `()` placeholder and refuses when today's note has an
open timed Pomodoro. Whole-item [`=name` starts](#equals-named) restart or swap.

Contract: [Starting the session atomically](../capture.md#starting-the-session-atomically).

### `@route:block-id` with no other text {#link-existing}

Link the existing `^block-id` task in `route.md` into today's ledger. No
task is created. A task Blocked only by its own prerequisites drops those
links first, then the gesture runs. `=<X>` starts the resolved session.

```bash
bob capture '@work:outline'
```

```text { .no-copy title="Output" }
✓ link  work.md
  [ ] → [*]  Write an outline  ^outline
  Linked under WORK
```

```bob-task
- [*] #task Write an outline [fresh:: 2026-10-20] [created::2026-10-20] ^outline
```

The Task Link is a dedicated child under the open Pomodoro:

```markdown
- [ ] () — WORK
	- [[work#^outline]]
```

Contract: [Linking and starting existing tasks](../capture.md#linking-and-starting-existing-tasks).

### `^route:block-id` {#caret-active-task}

Identical execution to the `@` link forms. `^` is the active-task spelling
and completes In Progress and Next tasks when that is the gesture.

```bash
bob capture '^work:outline'
```

```text { .no-copy title="Output" }
✓ link  work.md
  [ ] → [*]  Write an outline  ^outline
  Linked under WORK
```

Contract: [Linking and starting existing tasks](../capture.md#linking-and-starting-existing-tasks).

## Task pickers

### `:<query>` {#colon-picker}

Incomplete task-link picker (`task_link`). Accepting inserts
`@route:block-id`; the query stays uncaptured.

```bash
bob capture ':queue'
```

```text { .no-copy title="Output" }
bob capture: capture item 1 starting on line 1: `:queue` opens the task picker; pick a task to insert its `@<route>:<block-id>` link, or write the link yourself (for example `@sase:deep-fix`)
```

Contract: [Picking any open task with ':'](../capture.md#picking-any-open-task-with).

### `/<query>` {#slash-picker}

Incomplete Go To picker (`task_goto`); accepting opens tasks in Obsidian
without capture.

```bash
bob capture '/queue'
```

```text { .no-copy title="Output" }
bob capture: capture item 1 starting on line 1: `/queue` opens the Go To picker; pick a task to open it in Obsidian — it never captures
```

Contract: [Going to a task with '/'](../capture.md#going-to-a-task-with).

### `+query` {#plus-parent-picker}

Incomplete: choose a parent task. `capture-parse` needs `task_parent`.
Accepting inserts `@route+block-id`. Terminal prose `+` is the same picker.

```bash
bob capture-parse -f json -- '+prep'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"incomplete","needs":["task_parent"],"body":""}
```

Contract: [Sub-bullets under existing tasks](../capture.md#sub-bullets-under-existing-tasks).

### lone `+` {#lone-plus}

Opens the same parent picker in an editor. Escape leaves the valid `+5m`
Pomodoro adjustment unchanged. A submitted lone `+` is the five-minute
adjustment, not this picker.

```bash
bob capture --dry-run '+'
```

```text { .no-copy title="Output" }
[dry-run] ok would adjust  2026/20261020.md
  WORK 1200-1215 (15m) to 1200-1220 (20m), +5m at line 8
```

Contract: [Grammar at a glance](../capture.md#grammar-at-a-glance).

## Prerequisites and blocked-task links

### `&note:block-id` {#ampersand-prereq}

The captured (or explicitly selected) task depends on `note.md`'s
`^block-id` task. Nested notes use `&projects/foo:bar`; notes with spaces use
`&"Shopping List":bar`.

```bash
bob capture 'Review the intake queue @work^queue &work:prep'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
  ⛓ depends on "Prep the intake"  work.md ^prep  [ ] Ready
```

```bob-task
- [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
```

The source of truth is the managed Depends-On child, not the field:

```markdown
- [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
	- ⛓️ **DEPENDS ON:** [[#^prep]]
```

A selected `&note:block-id` with no owner is incomplete
(`dependency_target`): type task text or `+` to choose an existing owner.

Contract: [Adding prerequisites with '&'](../capture.md#adding-prerequisites-with).

### `&` / `&query` {#ampersand-picker}

Incomplete: the prerequisite picker is still open (`task_dependency`).
`&query` and `&note:` are the same incomplete family.

```bash
bob capture-parse -f json -- '&prep'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"incomplete","needs":["task_dependency"]}
```

Contract: [Adding prerequisites with '&'](../capture.md#adding-prerequisites-with).

### `^route:block-id &note:id` {#add-prereq-existing}

Add a prerequisite to an existing open owner. `^route:id` is a bare owner
alias here and does not create a Pomodoro link. `&note:id ^route:block-id`
is the same.

```bash
bob capture '^work:queue &work:prep'
```

```text { .no-copy title="Output" }
✓ related  work.md
  [ ] → [?]  Review the intake queue  ^queue
  ⛓ depends on "Prep the intake"  work.md ^prep  [ ] Ready
```

```bob-task
- [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
```

Contract: [Relating existing tasks](../capture.md#relating-existing-tasks).

### `*note:block-id` {#star-blocks}

The named open task depends on the captured (or explicitly selected) task.
Quote `*` on the shell. Nested notes and quoted names match `&`.

```bash
bob capture 'Ask for the dump @work^dump *work:queue'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [ ] #task Ask for the dump [created::2026-10-20] [id:: work__dump] ^dump
  🧱 blocks "Review the intake queue"  work.md ^queue  [?] Blocked
```

A line-initial bare `*` is never this marker. A selected star with no owner
is incomplete (`dependency_target`).

Contract: [Blocking existing tasks with '*'](../capture.md#blocking-existing-tasks-with).

### `*query` {#star-picker}

Incomplete: the blocked-task picker is still open (`blocked_task`).
`X *` and `X *note:` are the same family after other text on the line.

```bash
bob capture-parse -f json -- 'Ask for the dump *queue'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"incomplete","needs":["blocked_task"]}
```

Contract: [Blocking existing tasks with '*'](../capture.md#blocking-existing-tasks-with).

### `^route:block-id *note:id` {#star-existing}

Make the named open task depend on the existing owner. `^route:id` is a bare
owner alias and does not create a Pomodoro link.

```bash
bob capture '^work:dump *work:queue'
```

```text { .no-copy title="Output" }
✓ related  work.md
  🧱 blocks "Review the intake queue"  work.md ^queue  [?] Blocked
```

Contract: [Relating existing tasks](../capture.md#relating-existing-tasks).

## Relation links

### `~note:block-id[|"label"]` {#tilde-link}

The captured or selected task links to `note.md`'s `^block-id` task,
optionally labelled. Nested notes use
`~projects/foo:bar`; notes with spaces use `~"Shopping List":bar`;
labels use `~note:id|see` or `~note:id|"follow-up of"`. Quote `~` on
the shell. The target may be open, closed, in any active note, a daily
note, or `done/` (a mirror is skipped for archived and previous-daily
targets). The source owns the LINKS entry.
Configured cancel, complete, or merge labels also act on that source;
other labels record the link. Successor labels can offer the source when
a Bob action parks or completes the target; adding a link after that event
does not offer it. See [relation effects](configuration.md#relations).
`||` also hides the label from suggestions after capture succeeds.
Three pipes fail: `use || to hide a label, | to name one`.

The label pipe attaches directly to the target. Editor
[`capture-rewrite`](../capture.md#bob-capture-rewrite) with `--cursor`
attaches a spaced pipe across spaces or tabs (`~note:id |` → `~note:id|`,
`~note:id ||` → `~note:id||`); plain `bob capture` input applies no
rewrites. `Task =`/`~=`/`~~=` become `Task ~`/`~~`/`~`; after a complete
target a glued `~` or `=` flips direction, after a label only `~` flips,
and the label stays unchanged.

```bash
bob capture 'Fix flaky capture retries ~sase:retry-design|see'
bob capture 'Fix trip ~work:foo||prompted-by'
```

```text { .no-copy title="Output" }
✓ captured  mac_inbox.md
  🪢 links to "Design a retry policy" · see   sase.md ^retry-design  [ ] Ready
```

A selected `~note:block-id` with no owner is incomplete
(`dependency_target`): type task text or `+` to choose an existing
owner. A bare `~` / `~query` / `~note:` needs `relation_target`; a
label position (`~note:id|`, `~note:id||`) needs `relation_label`.

Contract: [Linking tasks with '~' and '~~'](../capture.md#linking-tasks).

### `~~note:block-id[|"label"]` {#tilde-tilde-link}

The named task links to the captured (or explicitly selected) task.
The captured task is the target, so it is always minted; the named
task's LINKS line must be editable (the same refusal set as `*`: daily
notes, `done/`, `_templates`, `_generated`, `_conflicts`,
dot-directories, fenced code).

```bash
bob capture 'Write the outline ~~sase_blog:prj|"next step"'
```

Contract: [Linking tasks with '~' and '~~'](../capture.md#linking-tasks).

### `^route:block-id ~note:id` {#tilde-existing}

Relate an existing owner. `^route:id` is a bare owner alias here and
does not create a Pomodoro link. `~note:id ^route:block-id` and
`^route:block-id ~~note:id` are the same family.

```bash
bob capture '^bob:fix-retries ~sase:retry-design|see'
```

Contract: [Linking tasks with '~' and '~~'](../capture.md#linking-tasks).

## Completion

### `!` / `!query` {#bang-picker}

Incomplete: pick an open task to complete (`task_complete`). Accepting
inserts `!<note>:<block-id>`. `!fix`, `!sase:`, and `!"Shopping List` are
still typing the query.

```bash
bob capture '!'
```

```text { .no-copy title="Output" }
bob capture: capture item 1 starting on line 1: `!` opens the task picker; pick a task to insert its `!<note>:<block-id>` token, or write one yourself (for example `!sase:fix-flaky`)
```

Contract: [Completing tasks with '!'](../capture.md#completing-tasks-with).

### `!note:block-id` {#bang-complete}

Complete that existing open task. The token must be the whole capture item.
`bob capture` closes it exactly as `x!N` would, retires its Task Links, and
unblocks dependents. Extra text is `invalid_task_complete`.

```bash
bob capture '!work:prep'
```

```text { .no-copy title="Output" }
✓ completed [ ] → [x] Prep the intake  work.md ^prep
```

```bob-task
- [x] #task Prep the intake [created::2026-10-20]  [completion:: 2026-10-20] ^prep
```

Contract: [Completing tasks with '!'](../capture.md#completing-tasks-with).

## Adjustments, shifts, and session start

### `+[N]` {#plus-adjust}

Extend today's current timed Pomodoro by N five-minute units (default 1).
`+5` is +25 minutes. The item must contain only the signed count.

```bash
bob capture '+2'
```

```text { .no-copy title="Output" }
✓ adjusted  2026/20261020.md
  WORK 1200-1215 (15m) to 1200-1225 (25m), +10m at line 8
```

Contract: [Adjusting the current Pomodoro](../capture.md#adjusting-the-current-pomodoro).

### `-[N]` {#minus-adjust}

Shorten by N units. Bare `-` is one unit (−5 minutes). Extra text after a
bare sign stays prose (`- foo`).

```bash
bob capture --dry-run '-2'
```

```text { .no-copy title="Output" }
[dry-run] ok would adjust  2026/20261020.md
  WORK 1200-1225 (25m) to 1200-1215 (15m), −10m at line 8
```

Contract: [Adjusting the current Pomodoro](../capture.md#adjusting-the-current-pomodoro).

### `++[N]` {#plus-plus-shift}

Shift a running timed Pomodoro later, keeping its duration (`++3` is +15
minutes). Use the operator alone; `+++` is prose.

```bash
bob capture --dry-run '++3'
```

```text { .no-copy title="Output" }
[dry-run] ok would shift  2026/20261020.md
  WORK 1200-1225 (25m) to 1215-1240 (25m), +15m at line 8
```

Link-start body rules: [linking and starting existing tasks](../capture.md#linking-and-starting-existing-tasks).

### `--[N]` {#minus-minus-shift}

Shift earlier by N units (default 1); pass bare `--` after the option delimiter.

```bash
bob capture --dry-run -- '--'
```

```text { .no-copy title="Output" }
[dry-run] ok would shift  2026/20261020.md
  WORK 1200-1225 (25m) to 1155-1220 (25m), −5m at line 8
```

Contract: [Shifting the current Pomodoro](../capture.md#shifting-the-current-pomodoro).

### `=` / `=<X>` {#equals-start}

Start today's next future Pomodoro now with `se<X>` timing (`=` is 25
minutes, `=3` is 15, `=-2` is 25 minutes with a 10-minute earlier offset,
`=+2` is 25 minutes with a 10-minute later offset), or
restart the running session with that timing. A restart prints an override
warning. The item must contain only the start token. An empty-suffix start of
a `MEET` session takes the span of its next back-to-back Event Tasks instead
and writes `[off:: Nm]` when that run is off-clock: see [MEET starts follow
the calendar](../capture.md#meet-starts-follow-the-calendar). Every other
start stamps `[t::]` or `[off::]` from the counted-links rule: see
[Which clock a session is on](../capture.md#which-clock-a-session-is-on).

```bash
bob capture '=3'
```

```text { .no-copy title="Output" }
✓ started  2026/20261020.md
  WORK 1200-1215 (15m) at line 8
  - [ ] (**1200-1215** [t:: 15m]) — WORK
  1 [*] Write an outline work.md ^outline
```

Contract: [Starting the next Pomodoro](../capture.md#starting-the-next-pomodoro).

### `=name` / `=<X>name` {#equals-named}

Start with `=focus`, `=3focus`, or `=focus3`; timing can precede or follow a
bare name, starting with a letter, containing no `-`, ending in no digit
or `+`, and not close-shaped. Use `#` (`=3#deep-work`, `=#2fa`, `=#c++`) or
exact literals for other names; timing goes before `#`. An open match starts
in place (whole slug before prefix); a completed match starts a new session
("again"); otherwise Bob creates and starts it. With a session running, a
different name swaps in: no `<X>` keeps its clock, `<X>` supplies fresh
timing.

```bash
bob capture '=focus'
```

```text { .no-copy title="Output" }
✓ started  2026/20261020.md
  FOCUS 1200-1225 (25m) at line 8
  - [ ] (**1200-1225** [t:: 25m]) — FOCUS
```

#### Exact Pomodoro names

`#"NAME"` matches the complete normalized name, preserving spaces and
supported punctuation instead of using slug/prefix matching. For example,
`DEEP WORK` and `DEEP-WORK` stay distinct. The literal also works in link
suffixes. Whitespace collapses and ASCII letters become uppercase. Straight
or curly double quotes are accepted; an unclosed quote is incomplete input.

```bash
bob capture --dry-run '=#"DEEP WORK"'
bob capture --dry-run '@work:outline#"DEEP WORK"'
```

The inner double quotes are part of Bob's syntax; the outer single quotes
preserve them in the shell. The `#` is required: `'="DEEP WORK"'` captures
prose. Completion uses exact literals when a slug would lose the spelling
or select a different name.

Contract: [Starting a named Pomodoro](../capture.md#starting-a-named-pomodoro)
and [Lossless acceptance](../task-types.md#pt8-lossless-acceptance).

### `=/[base][#type…]` {#equals-split}

`=/` splits the current Pomodoro: the open timed entry, else the next
planned entry. Its base (first type component) stays; every other type gets
an untimed planned entry below it, in source-name order, with that type's
Task Links and notes. The source keeps its timing and other notes.
`=/build` selects an open merged entry based at BUILD, with no fallback to
the current entry. With three or more types, `=/build#fix[#read]` selects
only those; with two, omit `#type`. MEET and single-type entries cannot
split; selectors accept quoted literals.

```bash
bob capture --dry-run '=/build'
```

Contract: [Splitting a merged Pomodoro](../capture.md#splitting-a-merged-pomodoro).

### `=*[source][#destination…]` {#equals-merge}

`=*build#fix` keeps BUILD and absorbs FIX. The name before `#` selects the
surviving source; names after `#` select entries to remove (`=*build#fix#read`
absorbs FIX, then READ). `=*#fix` uses the open timed entry as source, else
the next planned entry. Without a `#`, `=*` or `=*build` absorbs the first
open, untimed, named Pomodoro below its source, passing completed, timed,
running, unnamed, and MEET entries without wrapping above it; one entry per
token. The source may be timed or planned; it keeps its timing and ledger
order and gains the absorbed names and children; an absorbed Task Link
duplicating a source line is
removed with its subtree. Named absorbed entries must be untimed and open;
MEET cannot merge, and a trailing `#` still needs a destination name.

```bash
bob capture --dry-run '=*'
```

Contract: [Merging Pomodoros](../capture.md#merging-pomodoros).

### `~<K>` {#tilde-drop}

On a start token, drop the queued Task Links in `<K>` (`=~2`, `=3~2,4`,
`=bugs~2`). `~` always comes last. `=~` and `=~2,` are incomplete
(`pomodoro_start_task`). A start cannot complete links; `!` belongs on `x`.

```bash
bob capture --dry-run '=~2'
```

```text { .no-copy title="Output" }
[dry-run] ok would start  2026/20261020.md
  WORK 1200-1225 (25m) at line 10
  dropped link 2
```

```bash
bob capture-parse -f json -- '=~'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"incomplete","needs":["pomodoro_start_task"]}
```

Contract: [Starting the next Pomodoro](../capture.md#starting-the-next-pomodoro).

## Closing, outcomes, and work logs

### `x` {#x-close}

`x` closes today's timed Pomodoro. Numeric lists defer unlisted links; Event Tasks keep
[clock outcomes](../capture.md#event-task-links-close-by-the-clock).
Authored markers stay. Order is free; `0` selects none; `@<N>` keep-list last.

```bash
bob capture --dry-run 'x'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] → [/] Write an outline work.md ^outline
  next: WORK (created) at line 10 · carries 1 link
```

Mid-body `x` is prose. Grammar/paste/`capture-parse`/`bob capture` never close
`xx`, `x-ray`, or capital `X`; Mac typing maps eligible `xx`/`xs` to `x!`/`x*`
([sigil alias](../capture.md#bob-capture-rewrite)).

Contract: [Closing the running Pomodoro](../capture.md#closing-the-running-pomodoro).

### `[N]x` {#nx-close}

`[N]x` sizes the `x` close: `2x` closes at exactly 2 five-minute blocks (10
minutes) from the session start, with the same selection as `x`. One or two
digits, never a task selection. An unelapsed size, an unstarted session, or
an unparseable range refuses with a fix and writes nothing.

```bash
bob capture-parse -- '2x!2'
```

```text { .no-copy title="Output (excerpt)" }
  body      2x!2
  close     2x!2 (10m · complete 2 · defer the rest)
```

A capital `X` never sizes (`2X4 lumber` stays a task), and `2x-ray`, `2.5x`,
and `2 x!2` stay prose. Three or more digits (`100x`) stay reserved.

Contract: [Closing at a size](../capture.md#closing-at-a-size).

### `x!<M>` {#x-complete-links}

`x!1` completes task 1, deferring other ordinary plain links. Empty `!`
completes remaining links except not-started Event Tasks.

```bash
bob capture --dry-run 'x!'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] → [x] Write an outline work.md ^outline
```

```bob-task
- [x] #task Write an outline [created::2026-10-20] [completion:: 2026-10-20] ^outline
```

Contract: [Choosing each Task Link's outcome](../capture.md#choosing-each-task-links-outcome).

### `x*<P>` {#x-park-links}

Park those links: worked tasks become or remain Pending (`[/]`) without
carrying their links into the next session. Empty `*` (`x*`) parks the
remaining numbered links, except Event Tasks whose meeting has not started.

```bash
bob capture --dry-run 'x*'
```

Tasks whose `starts after` relation points to a parked task with no
remaining link can be offered as successors, linking only when selected with
`--pull-in`.

Contract: [Choosing each Task Link's outcome](../capture.md#choosing-each-task-links-outcome).

### `x~<K>` {#x-drop-links}

Drop listed links: removed, not carried or started; other ordinary plain links
defer. `x~` is incomplete (`pomodoro_close_task`).

```bash
bob capture --dry-run 'x~1'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] dropped  Write an outline work.md ^outline
```

```bash
bob capture-parse -f json -- 'x~'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"incomplete","needs":["pomodoro_close_task"]}
```

Contract: [Choosing each Task Link's outcome](../capture.md#choosing-each-task-links-outcome).

### Work Log {#x-work-log}

Write one inline entry or child bullets. Without wildcards, explicit lists assign
unnumbered bullets to listed worked tasks in order; one task takes all. `x!2 note`
logs to 2; `x~2 note` fails. A leading number selects a task; `x 1` needs
`pomodoro_close_log_text`. Entries gain `*YYYY-MM-DD HH:MM* —` stamps; number
bullets all or none, `- * summary` logs to remaining worked tasks, and
two-space bullets nest undated details.

```bash
bob capture --dry-run 'x wired the lexer'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] → [/] Write an outline work.md ^outline
  logged: wired the lexer
```

Contract: [Logging work while closing](../capture.md#logging-work-while-closing).

### `+2 x` / `x =` {#operator-chain}

Whitespace-separated session tokens on one line run left to right exactly
like blank-line items (`+2 x`, `x =`, `x =bugs`, `=/ =`). An inline
Work Log entry sits right after its close, with operators trailing
(`x wired the lexer =`). Child bullets attach to that line's `x`.

```bash
bob capture --dry-run '+2 x'
```

```text { .no-copy title="Output" }
[dry-run] ok would adjust then close  2026/20261020.md
  WORK 1200-1215 (15m) to 1200-1225 (25m), then close
```

Contract: [Chaining session operators on one line](../capture.md#chaining-session-operators-on-one-line).

### `@route:block-id=x` {#link-then-close}

Put that existing task into the running session, then close it. Own-prerequisite
Blocked targets drop those links first. The `=` is the suffix separator;
numbers refer to the post-link lineup. `^route:block-id=x` is the
active-task spelling. With extra text, create the new linked task and then
close.

```bash
bob capture --dry-run '@work:outline=x'
```

```text { .no-copy title="Output" }
[dry-run] ok would link then close  work.md ^outline
  [ ] → [*] Write an outline, then close WORK
```

Contract: [Closing the running Pomodoro](../capture.md#closing-the-running-pomodoro).

## Project captures

### `@route^block-id@` {#project-note-seal}

Create `<route>_<block_id>.md` (hyphens in the ID become underscores). `^prj`
is never linked. `#pomodoro` sends ` :id` links under that named Pomodoro.
With `%n`, this caret seal creates a notebook.

```bash
bob capture '@work^intake-flow@' 'Ship the intake flow'
```

```text { .no-copy title="Output" }
✓ captured  work_intake_flow.md
  parent  [[work]]
  - [ ] #task #prj Ship the intake flow #hide ^prj
  1 task · sections —
```

```bob-task
- [ ] #task #prj Ship the intake flow #hide ^prj
```

Contract: [Project notes](../capture.md#project-notes).

The `@` seal marks a new project note. Editors running
[`capture-rewrite`](../capture.md#bob-capture-rewrite) switch sealed forms
with `:`/`^` right after the seal, `+` after `@route^id` gives `@route+id`,
and `- .`/`- ;`/`- ^.` rewrite the same way on Move bullets.

### `@route:block-id@` {#project-note-offer}

Create the same project note, then **offer** its open tasks as successors of the
new `^prj` task. Nothing is linked until you pick it: `-p 1,3`,
`-p <key>`, or `-p all` link the picked rows exactly as if each had been
authored as a ` :<id>` child, and the default (or `-p none`) links only the
explicit ` :` children. `--require-pull-in-choice` refuses write-free while
offers are unanswered. A picked task is **not** stamped fresh. `#pomodoro`
after the seal sends explicit links and picks under that Pomodoro, which is
created only when something lands there.

```bash
printf 'Ship the intake flow @work:intake-flow@\n- Name the stages\n- Draft the memo\n' \
  | bob capture --dry-run
```

```text { .no-copy title="Output" }
[dry-run] ok would capture  work_intake_flow.md
  parent  [[work]]
  - [ ] #task #prj Ship the intake flow #hide ^prj
  2 tasks · sections —
  New project work_intake_flow.md
  offer 1  unblocked [ ] → [ ] Name the stages  work_intake_flow.md
  offer 2  unblocked [ ] → [ ] Draft the memo  work_intake_flow.md
  hint: 'bob projects sync' adds the parent's Sub-projects line; 'bob task reconcile' reconciles Blocked state
  -p 1,2 · -p all
```

Contract: [Offer a new project's tasks](../capture.md#offer-a-new-projects-tasks).

### `:id` child {#project-colon-child}

On a project-note capture, `- <task> :<task-id>` names the task, makes it
Next (`[*]`, or `[?]` when scheduled), and links `[[<stem>#^<task-id>]]`
into the current/next Pomodoro, or the `#pomodoro` one.

```bash
bob capture '@work^intake-flow@' 'Ship the intake flow' '- Name the stages :stages'
```

```bob-task
- [*] #task Name the stages [created:: 2026-10-20] ^stages
```

The child is Next and linked from today's ledger as
`[[work_intake_flow#^stages]]`.

Contract: [Project notes](../capture.md#project-notes).

### `^id` child {#project-caret-child}

On a project-note capture, `- <task> ^<task-id>` names the task only:

```bob-task
- [ ] #task Named child [created:: 2026-10-20] ^named-child
```

Contract: [Project notes](../capture.md#project-notes).

## Task children and link toggles

### `@route+block-id` {#plus-child}

Ordinary child bullet under an existing task. Editors that run
[`capture-rewrite`](../capture.md#bob-capture-rewrite), including the Mac
capture panel, turn `+` typed immediately after `@route:block-id` into
`@route+block-id`.

```bash
bob capture 'Check the parser @work+prep'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  under [ ] Prep the intake  ^prep
  - Check the parser
```

A note-less `@route+id` with only `%c` files the clipboard as notes directly
under that task: `bob capture '@work+prep %c'`.

Contract: [Sub-bullets under existing tasks](../capture.md#sub-bullets-under-existing-tasks).

### Target run {#plus-child-target-run}

A `@route+id @route+id` run applies one capture to several tasks, in marker order.

```bash
bob capture '@work+one @work+two Ship it'
```

```text { .no-copy title="Output" }
targets  @work+one · @work+two
```

### `>` transfer {#transfer}

Move or merge whole tasks: `@route+id` sources, `>`, then the
destination — a bare `@route` moves, the rest merge. Quote the draft so
the shell keeps the `>`.

```bash
bob capture '@inbox+bench @work+pgdump > @db'
bob capture '@inbox+bench @work+pgdump > @db+upgrade'
bob capture '@inbox+bench > @db+upgrade#future-work'
bob capture '@mac_inbox+ws-proto > @dev#future-work'
```

```text { .no-copy title="Output" }
↪ Moved 2 tasks into db.md › Tasks
⤵ merged 2 tasks into “Upgrade Postgres to 17” · db.md
```

```text { .no-copy title="Shapes" }
@a+x @b+y > @c        move into a note
@a+x @b+y > @c+id     merge into a task
@a+x > @c+id#drawer   merge into a drawer
@a+x > @c#section     merge into a section
```

Contract: [Moving and merging tasks with `>`](../capture.md#moving-and-merging-tasks-with).

### `<` merge-in {#merge-in}

`T < @a+x @b+y` creates T, then merges each source into it.

```bash
bob capture 'Some great new task! @foo^great < @foo+f @bar+b'
```

```text { .no-copy title="Shapes" }
T < @a+x @b+y   merge sources into the new task T
T <             pick tasks to merge in (needs `merge_in_source`)
```

Contract: [Merging tasks into a new task with `<`](../capture.md#merging-tasks-into-a-new-task-with).

### `@route+block-id#drawer` {#plus-child-section}

Child bullet inside a task drawer: an ALL-CAPS child bullet holding nested
notes. `@route+block-id#section` targets `SECTION`. The glued `#drawer`
requires note text, child bullets, or `%c`. Names match by normalized key;
`#future-work` matches `FUTURE WORK`. Missing drawers are created, but a
prefix or near-duplicate name is refused. A quoted `#"FUTURE WORK"` permits
deliberate creation near an existing name. On a project's `^prj`, drawers
are eligible `##` sections. Managed logs and structural headings are reserved.

```bash
bob capture 'Need a parser @work+prep#requirements'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  under [ ] Prep the intake  ^prep  REQUIREMENTS
  - Need a parser
```

Contract: [Sub-bullets under existing tasks](../capture.md#sub-bullets-under-existing-tasks).

### Drawer bullets and several notes {#drawer-bullets}

A first-level `- #requirements` under a task, with nested notes, names a
drawer (ALL-CAPS with nested notes is the same). Empty `- #drawer` is
refused; empty ALL-CAPS stays a note. Under `@nb` or `%n`, `- #Section`
names a `##`; nest notes or write `\#git`.

A note-less `@route+id` files each first-level bullet as a separate note;
drawer bullets append inside their drawers. With `@route+id#drawer`, every
bullet files inside that drawer. Same-line note text keeps its children
nested under that one note.

Contract: [Several notes at once](../capture.md#several-notes-at-once).

### `@route+block-id` with no other text {#plus-ensure-next}

With no note text or child bullets, `@route+block-id` ensures Next or preserves
In Progress and moves an existing Task Link within today's open Pomodoros.
Placement uses the task's effective type (explicit or inherited); with none,
it uses today's current/next Pomodoro. A separate `#type` retypes first:
`@work+outline #fix`. Without an existing link it refuses; `@work+outline!`
adds one when eligible. Use `^route:block-id#NAME` to move an existing link
to a named destination. Tasks blocked only by their own prerequisites drop
those dependencies first.

```bash
bob capture '@work+outline'
```

```bob-task
- [*] #task Write an outline [created::2026-10-20] ^outline
```

Contract: [Task Link toggle](../capture.md#task-link-toggle).

### `@route+block-id!` {#plus-toggle}

Toggle that task's Pomodoro Task Link: link it when unlinked (Ready and
own-prerequisite Blocked rise to Next; other Blocked stay unlinked) or
unlink it when linked (lane unchanged). Removing a Pending task's last link
under today's open Pomodoros can offer its `starts after` successors;
removing a Next task's link cannot.

```bash
bob capture '@work+outline!'
```

```text { .no-copy title="Output" }
✓ toggle  work.md
  [ ] → [*]  Write an outline  ^outline
  linked under WORK
```

Contract: [Task Link toggle](../capture.md#task-link-toggle).

## Pomodoro notes

### `%n` {#trailing-hash}

`%n` at an item edge is a Pomodoro note, not a routed task. On `@parent^id@`
it creates a notebook.

```bash
bob capture 'remembered the timeout %n'
```

```text { .no-copy title="Output" }
✓ captured  2026/20261020.md
  under (**1200-1230** [t:: 30m]) — WORK
  - remembered the timeout
```

Contract: [Pomodoro notes](../capture.md#pomodoro-notes).

## Scheduling, priority, and clipboard

### `s:<N>` {#s-schedule}

`[scheduled::]` N days from today, recognized in the terminal token region.
Checkbox-bearing captures start Blocked (`[?]`), including `s:0`.

```bash
bob capture 'File taxes @work s:1'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [?] #task File taxes [created::2026-10-20] [scheduled::2026-10-21]
```

```bob-task
- [?] #task File taxes [created::2026-10-20] [scheduled::2026-10-21]
```

Contract: [Scheduling and priority](../capture.md#scheduling-and-priority).

### `$` / `$<N>` {#dollar-priority}

Write priority level N (bare `$` = 1) and roll a scheduled date in that
level's window. A missing config file is an error; there is no built-in
default. An explicit `s:<N>` wins the date; `$<N>` still writes priority.
Mid-line amounts stay text (`Pay $3 to Bob`).

```bash
bob capture-parse -f json -- 'File taxes @work $1'
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":1,"mode":"task","route":"work","body":"File taxes"}
```

A configured P1 window writes the priority field and rolls `[scheduled::]`
in that window. This illustration is the resulting task text, not a captured
envelope:

```bob-task
- [?] #task File taxes [created::2026-10-20] [priority:: high] [scheduled::2026-10-21]
```

Contract: [Scheduling and priority](../capture.md#scheduling-and-priority).

### Bare `%` {#percent-picker}

A bare `%` is an incomplete directive picker. In an editor, it opens a menu with
Clip (`%c`), Pomodoro note (`%n`), Template (`%t`), and Listen (`%l`) for a
bare-URL reference. The rows show the Pomodoro target and configuration status;
unavailable choices explain why and are omitted by shell completion without
reading clipboard content. In Mac Capture, `\` where `%` would
open the directive menu becomes `%` (not at a line start); type `\` `\` `%`
for `\%`.

Contract: [Directives (`%`)](../capture.md#directives).

### `%c` / `%c<N>` {#percent-clipboard}

Capture clipboard content as child bullets. `%c` reads the live clipboard
once; `%c3` reads three values. `-c, --clip` forces a clipboard read as if
`%c` were written. `-n, --no-clip` never reads the clipboard: a dry run
reports `clip.status: not_read`, and a real capture refuses. Mid-line `50%`
stays prose. Clipboard backends are stubbed in this isolated fixture, so
the example keeps the percent as body text.

```bash
bob capture 'Keep 50% @work'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [ ] #task Keep 50% [created::2026-10-20]
```

Contract: [Directives (`%`)](../capture.md#directives) · [Clipboard](../capture.md#clipboard).

### Bare `#` {#hash-picker}

A bare `#` at an item edge is an incomplete type marker. In an editor it opens
task-type completion, with rows in configured type order (or recent catalog
order) and no row selected. Write `\#` to keep a literal hash. In Mac Capture,
`/` where `#` would open a menu becomes `#` (not at a line start);
doubling it leaves one literal.

Contract: [Typing tasks with `#<type>`](../capture.md#typing-tasks-with-type).

### `#<type>` {#hash-type}

The clock spellings `#off`, `#on`, `#offclock`, `#off-clock`, and
`#untracked` are reserved task tags at either item edge: never type markers or
completion choices, kept verbatim on new tasks. On an item that selects an
existing task, capture refuses them; edit that task line directly.

Type a new task at either item edge (`#fix Fix it`, `Fix it #fix`), or retype
an existing task the item targets. Slugs start with an ASCII letter and use
letters, digits, and hyphens; quoted literals allow spaces and punctuation
(`#"DEEP WORK"`). A leading `# Heading text` is incomplete;
`\# Heading text` is literal.
`#` plus a digit, `##…`, `#-…`, mid-line `#`, and glued hashes stay ordinary
prose. `t:` is ordinary prose. Escape `\#fix` to keep the literal hash text
in the body.

With configured `types:`, only configured entries are accepted. Without that
vocabulary, only names already present in Pomodoro history are accepted;
inventory labels are excluded. Unknown names fail transactionally with
`type_not_configured`, including in dry-run mode. A separate `#type` token on
`@route+block-id` retypes the target in the same transaction; with no body or
authored children it also ensures Next, while with body text the glued
`#drawer` suffix keeps its task-drawer meaning. `@route^block-id #type` (separate
token) types the new task with that explicit block ID. Named Pomodoro owners such as `@route:block-id#NAME`,
`^route:block-id#NAME`, `=NAME`, and project-note `@route^block-id@#NAME` /
`@route:block-id@#NAME` keep their existing meanings.

With a Pomodoro destination, the task keeps its effective type (including a
separate `#type` choice), and a typed destination gains a missing type: a
BUILD task landing in FIX makes `FIX + BUILD`. A container (a name with no
type components) keeps its name and the task's type, with a warning; a MEET
destination retypes a different effective type to MEET. Explicit retyping
stages the `[type::]` edit with the gesture and relocates the live Task
Link. Closed targets are refused.

```bash
bob capture --no-clip 'Fix the parser #fix @sase'
```

```text { .no-copy title="Output" }
✓ captured  sase.md
  - [ ] #task Fix the parser [type:: FIX] [created::2026-10-20]
  type   → FIX · from sase suggests WRITE
```

Contract: [Typing tasks with `#<type>`](../capture.md#typing-tasks-with-type).

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Capture contract](../capture.md)
- [Vault format](vault-format.md)
- [bob capture CLI](cli/capture.md)
- [Capture](glossary.md#capture) and [Bulk Capture](glossary.md#bulk-capture)
- [Task dependency links](../task-dependencies.md)
- [Plan budget, Today, and lanes](../plan.md)
