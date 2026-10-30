---
title: Capture syntax
description: Index of bob capture tokens with syntax, examples, and links into the capture contract.
---

# Capture syntax

`bob capture` reads one draft and writes tasks, bullets, reference jobs, or
Pomodoro session changes. This page is the token lookup. The full grammar and
refusal messages live in the [Capture contract](../capture.md).

Examples below were captured from this checkout's `bob` in an isolated
`HOME`/`XDG_*` tree, a synthetic vault, `BOB_NOW=2026-10-20 12:00:00`,
`TZ=UTC`, and `BOB_REF_JOBS_KICK=off`. Copy the bundled demo with the
[first Bob day tutorial](../start/first-bob-day.md) when you need that
fixture. Session examples need today's daily note with an open
`## Pomodoros` entry. Dependency writes need Obsidian Tasks settings that
define Blocked as `?`. Quote tokens in Zsh (`*`, `!`, `?`, and `#` glob).
Zsh's `EQUALS` expansion rewrites a word that starts with `=`, so `bob capture =focus` fails unquoted: always single-quote (`'=focus'`, `'x =focus'`).
Network, clipboard, sync, and hook execution are not used here.

Options precede the draft. An unknown alphabetic long option such as
`--anything` is rejected before draft text begins; after text begins it is
literal. `--` ends option parsing: `bob capture -- '--anything'` captures
that text, and `bob capture --dry-run -- '--'` previews the bare shift token.

## Token index

Each row is a distinct operation. Equivalent spellings share a heading.
Incomplete picker input is labeled **picker** and is refused if you submit it.

| Token | Meaning | Heading |
| --- | --- | --- |
| Bare public URL | Queue a reference job under `mac_inbox` | [Bare public URL](#bare-public-url) |
| `URL @route` / `@route URL` | Queue the reference under that route | [`URL @route`](#url-at-route) |
| `URL %` / `URL % @route` | Queue with companion audio | [`URL %`](#url-listen) |
| `-r route` | Force the URL (or task) route from the CLI | [`-r`](#cli-route) |
| `@@route` | Shared task destination for unrouted items | [`@@route`](#global-route) |
| `@@route+block-id` | Shared parent-task destination | [`@@route+block-id`](#global-parent) |
| `@@template:` | Expand one capture per template row | [`@@template:`](#template) |
| `@route` | Write a task to `<route>.md` | [`@route`](#at-route) |
| `@route#Section` | Ordinary bullet under a matching heading | [`@route#Section`](#at-route-section) |
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
| `=name` | Start a named Pomodoro (`=bugs`, same token as `=#bugs`) | [`=name`](#equals-named) |
| `~<K>` | Drop those queued Task Links from the start | [`~<K>`](#tilde-drop) |
| `x` | Close today's running timed Pomodoro | [`x`](#x-close) |
| `x!<M>` | Close, completing those Task Links | [`x!<M>`](#x-complete-links) |
| `x*<P>` | Close, parking those Task Links | [`x*<P>`](#x-park-links) |
| `x~<K>` | Close, dropping those Task Links | [`x~<K>`](#x-drop-links) |
| Work Log | Log work onto the close's worked tasks | [Work Log](#x-work-log) |
| `+2 x` / `x =` | Chain session operators on one line | [Operator chain](#operator-chain) |
| `@route:block-id=x` | Link that task, then close | [`@route:block-id=x`](#link-then-close) |
| `@route^block-id+` | Create a project note | [`@route^block-id+`](#project-plus) |
| `:id` | Project-note child: name, Next, and link | [`:id` child](#project-colon-child) |
| `^id` | Project-note child: name only | [`^id` child](#project-caret-child) |
| `@route+block-id` | Child bullet under an existing task | [`@route+block-id`](#plus-child) |
| `@route+block-id#section` | Child under that task's ALL-CAPS section | [`@route+block-id#section`](#plus-child-section) |
| `@route+block-id` with no other text | Ensure Next and move its Task Link | [Ensure Next](#plus-ensure-next) |
| `@route+block-id!` | Toggle that task's Pomodoro Task Link | [`@route+block-id!`](#plus-toggle) |
| trailing `#` | Plain-text note on the current Pomodoro | [trailing `#`](#trailing-hash) |
| `s:<N>` | Schedule N days from today; checkbox tasks start Blocked | [`s:<N>`](#s-schedule) |
| `$` / `$<N>` | Write priority N and roll a scheduled date | [`$`](#dollar-priority) |
| `%` / `%N` / `%header` | Capture clipboard content as child bullets | [`%`](#percent-clipboard) |
| `t:<type>` | Type the new task | [`t:<type>`](#t-type) |

## Overloaded markers

These characters mean different things in different positions. Read them by
what they attach to, not as one global operator.

| Marker | Context | Meaning |
| --- | --- | --- |
| `+` | `+[N]` alone | Adjust the current Pomodoro |
| `+` | `++[N]` alone | Shift the running session later |
| `+` | `+query` / lone `+` | Parent-task picker |
| `+` | `@route+block-id` | Child under an existing task, or Ensure Next when the item has no other text |
| `+` | `@route^block-id+` | Create a project note |
| `+` | after `#name` | Part of the Pomodoro name (`BUGS+`) |
| `#` | trailing bare `#` | Pomodoro note |
| `#` | `@route#Section` | Heading in that note |
| `#` | `@route+id#section` with body text | ALL-CAPS task section |
| `#` | `@route:id#pomodoro` / `=name` | Named Pomodoro (`=bugs` and `=#bugs` are one token) |
| `@` | `@route…` | Route / destination token |
| `@` | `@@route` | Draft-wide destination |
| `@` | `x@<N>` / `=name@<X>` | Timing or keep-list typed last (`=bugs@3` means `=3#bugs`) |
| `@` | `URL @route` | Explicit reference parent |
| `*` | `*note:block-id` | Blocked-task (star) link |
| `*` | `x*` | Park Task Links on close |
| `*` | line-initial bare `*` | Never a blocked-task marker |
| `~` | `~note:block-id[\|"label"]`, `~note:block-id\|\|"label"` | Relation link ([`~`](#tilde-link)); `||` hides the label from suggestions once the capture succeeds |
| `~` | `x~` / `=~` | Drop Task Links on close / drop on start |
| `~` | `~/path`, `~5m`, mid-line `~x:y` | Never a relation marker |
| `~~` | `~~note:block-id[\|"label"]` | Reverse relation link ([`~~`](#tilde-tilde-link)) |
| `~~` | `~~old~~` | Strikethrough, never a relation marker |
| `!` | `!note:block-id` | Complete an existing task |
| `!` | `x!` | Complete Task Links on close |
| `%` | `URL %` | Request companion audio |
| `%` | trailing `%` | Clipboard capture |

Contract: [Grammar at a glance](../capture.md#grammar-at-a-glance).

**Mnemonic: `~` on its own ties two tasks; `~` inside `x` / `=` drops.**

**`=` then a word starts that Pomodoro; `#` only after a number.** `=bugs` and `=#bugs` are one token; write `#` only when a number comes first (`=3#bugs`) or the name starts with a digit or reads as a close (`=#2fa`, `=#x2`).

## Incomplete input versus executable syntax

A picker token is a draft state, not a capture. `bob capture` and
`bob capture-parse` refuse it with a teaching error until the selection
becomes a concrete `@route:id`, `!note:id`, `&note:id`, `*note:id`,
`~note:id`, `~~note:id`, or
`@route+id`. Session tokens that still need a number or name (`x~`, `=~`, `x@`,
`=#`) are the same kind of incomplete input. A bare `=name` start is complete (`=bugs`); only the empty `=#` needs `pomodoro_name`.

A signed count with extra prose (`+2 more`, `=3 more`) is an error, not a
task. A sign or `=` followed by prose (`Plan +5`, `= foo`) stays ordinary
task text. Capital `X` never closes; `X *job:cleanup` is how you capture a
task that must start with the word `x`.

## Retired and rejected forms

These are not valid commands. Bob teaches the replacement.

| Typed | What happens |
| --- | --- |
| `==`, `==<X>`, `==#name` | Retired restart/swap; write one `=` (`=`, `=3`, `=bugs`) |
| `==x` | Error: close with `x` |
| `@route:id+` / `@route:id+#pomodoro` | Retired project-note forms; write `@route^id+` |
| `@route::id` | Retired ordinary block-ID form; write `@route^id` |
| `#now` / a free `#tag` next to `@route` | Retired; heading and Pomodoro `#` attach to the route token |
| `x#bugs` | Error: `x` always closes the running session; write `x =bugs` to close then start |

Contract: [Retired close spellings](../capture.md#retired-close-spellings).

## Routes and destinations

### `@route` {#at-route}

Write a `#task` into `<route>.md` at the vault root. Route names are
`A–Z a–z 0–9 _ -`, lower-cased. The default without a route is `mac_inbox`.

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

### `@@route` {#global-route}

A draft-wide destination for otherwise-unrouted items. It may sit anywhere.
A plain `@@route` also routes default (routeless) refs as source `global`.
`@@route+block-id` keeps refs as tasks.

```bash
bob capture 'Buy milk @@work'
```

Contract: [Global destination declaration](../capture.md#global-destination-declaration).

### `@@route+block-id` {#global-parent}

Same declaration, but unrouted items become direct children of that existing
task.

```bash
bob capture 'Check the parser @@work+prep'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  under [ ] Prep the intake  ^prep
  - Check the parser
```

Contract: [Global destination declaration](../capture.md#global-destination-declaration).

### `@route#Section` {#at-route-section}

Write an ordinary bullet (no checkbox) into a matching non-`Tasks` heading.

```bash
bob capture 'Remember the timeout @notes#Ideas'
```

```text { .no-copy title="Output" }
✓ captured  notes.md
  - Remember the timeout [created::2026-10-20]
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

```bash
bob capture -r work -- 'https://example.com/article'
```

Contract: [Command-line options](../capture.md#command-line-options) and
[Saving links](../capture.md#saving-links-to-your-reading-queue).

#### Inbox task destinations

A capture that changes an existing inbox task's status reports its top-level
task block as a route candidate; changing a nested task routes that whole block.
The CLI leaves unanswered candidates in place. Repeatable
`-i N=note` or `--inbox-route KEY=note` moves a candidate after the capture action;
`-i N=` keeps one, and `-i keep` keeps every otherwise unanswered candidate.
Numbers and keys come from that draft's `inbox_route` preview.

For an existing Ready task `^triage` in `mac_inbox.md`:

```bash
bob capture --dry-run -f json -- '@mac_inbox:triage'
bob capture --dry-run -i 'mac_inbox.md#^triage=work' -- '@mac_inbox:triage'
```

Both commands only preview. The second plans the link, then the move to
`work.md` and link repairs. Removing `--dry-run` from that second command
commits the action and move together; do not commit the first command before
answering. New tasks and blocks with no status changes produce no candidates.
Destinations exclude inboxes; stale keys or refused moves abort the batch.

Contract: [Routing inbox tasks out](../capture.md#routing-inbox-tasks-out).

## URL routing and audio

### Bare public URL {#bare-public-url}

When the URL routing policy admits it, a bare public URL queues a background
reference job under `mac_inbox`. The confirmation says reading queue; the
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

### `URL %` {#url-listen}

Queue the reference with companion audio (`ref.listen` true). Parent
selection matches a bare URL: default, `@@route` global, or `-r`. Exact
standalone `@` remains a compatible listen alias. `URL % @route` / `@route URL %`
sets an explicit parent.

```bash
bob capture --dry-run 'https://example.com/article % @work'
```

```text { .no-copy title="Output" }
[dry-run] ok would queue  example.com/article → work
  new to your library · reading task lands in work.md · file it later · listen
```

Contract: [Saving links to your reading queue](../capture.md#saving-links-to-your-reading-queue).

## Templates

### `@@template:` {#template}

The first line is `@@template:`. The following `{N}` template captures
exactly like its expansion: one item per later nonblank row (`{0}`-only
templates can pack several values on one line). `-E, --expand` prints the
expansion and exits before vault access.

```bash
printf '%s\n' '@@template:' 'Call {0} @work' '' 'Alex' 'Sam' | bob capture -E
```

```text { .no-copy title="Output" }
Call Alex @work

Call Sam @work
```

A successful real `@@template:` capture saves the template block (not the
rows) on the shelf. Manage it with `bob capture-templates`.

Editors that run [`bob capture-rewrite`](../capture.md#bob-capture-rewrite),
including the Mac capture panel, turn a typed `@@@` at the very start of a
draft into the `@@template:` header and put the caret on the next line.

Contract: [Bulk templates](../capture.md#bulk-templates).

## Block IDs and linked tasks

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

Same, and atomically start the selected session. `<X>` uses five-minute units:
`=3` means 15 minutes; empty `=` means 25 minutes. A bare suffix name
(`@work:outline=bugs`) means `@work:outline#bugs=`.

```bash
bob capture '@work:outline=3'
```

```bash
bob capture '@work:outline=bugs'
```

| Suffix on `@work:outline` or `^work:outline` | Session and timing |
| --- | --- |
| `=focus@3` | FOCUS, 15 minutes; equivalent to `=3#focus` or `#focus=3` |
| `=focus@-2` | FOCUS, 25 minutes; start is now minus 10 minutes, rounded up to a five-minute boundary |
| `=#2fa` | A name starting with a digit, using the explicit `#` form |
| `=focus@` | Incomplete: timing is still needed |
| `=3#focus@2` | Rejected: timing appears twice |

A link-and-start fills an open `()` placeholder or creates a started session.
It refuses while any timed Pomodoro in today's daily note is open. The whole-item [`=name`
start](#equals-named) has separate restart and swap behavior.

Contract: [Starting the session atomically](../capture.md#starting-the-session-atomically).

### `@route:block-id` with no other text {#link-existing}

Link the existing `^block-id` task in `route.md` into today's ledger. No new
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

Incomplete: pick any open task to link. `capture-parse` needs `task_link`.
Accepting inserts `@route:block-id`. Execution never captures the query.

```bash
bob capture ':queue'
```

```text { .no-copy title="Output" }
bob capture: capture item 1 starting on line 1: `:queue` opens the task picker; pick a task to insert its `@<route>:<block-id>` link, or write the link yourself (for example `@sase:deep-fix`)
```

Contract: [Picking any open task with ':'](../capture.md#picking-any-open-task-with).

### `/<query>` {#slash-picker}

Incomplete: pick any open task to open in Obsidian. `capture-parse` needs
`task_goto`. Accepting opens the task; execution never captures it.

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

This is a draft state, not a successful capture.

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

This is a draft state, not a successful capture.

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

This is a draft state, not a successful capture.

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

The captured (or explicitly selected) task links to `note.md`'s
`^block-id` task, with an optional source-side label. Nested notes use
`~projects/foo:bar`; notes with spaces use `~"Shopping List":bar`;
labels use `~note:id|see` or `~note:id|"follow-up of"`. Quote `~` on
the shell. The target may be open, closed, in any active note, a daily
note, or `done/` (a mirror is skipped for archived and previous-daily
targets). Relations are inert: no status, lane, Pomodoro, or field
changes. A `||` spelling (`~note:id||see`,
`~note:id||"follow-up of"`) stores the identical label and hides it
from suggestions once the capture succeeds; a third pipe is a teaching
error (`use || to hide a label, | to name one`).

The label pipe attaches directly to the target. Editors using
[`capture-rewrite`](../capture.md#bob-capture-rewrite) with `--cursor` just
after a newly typed pipe can attach it across spaces or tabs
(`~note:id |` → `~note:id|`, `~note:id ||` → `~note:id||`); direct
`bob capture` input does not apply that rewrite.

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

Shift the running timed Pomodoro N units later, keeping its duration.
`++3` is +15 minutes. The item must contain only the operator. `+++` stays
prose.

```bash
bob capture --dry-run '++3'
```

```text { .no-copy title="Output" }
[dry-run] ok would shift  2026/20261020.md
  WORK 1200-1225 (25m) to 1215-1240 (25m), +15m at line 8
```

Contract: [Shifting the current Pomodoro](../capture.md#shifting-the-current-pomodoro).

### `--[N]` {#minus-minus-shift}

Shift N units earlier. Bare `--` is one unit; pass it after the option delimiter.

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
minutes, `=3` is 15, `=-2` is 25 minutes with a 10-minute offset), or
restart the running session with that timing. A restart prints an override
warning. The item must contain only the start token.

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

### `=name` / `=<X>#pomodoro` {#equals-named}

Start the named Pomodoro now (`=focus`, same token as `=#focus`; equivalent spellings `=<X>#pomodoro`, `=name@<X>`). An open match (whole slug, else prefix)
starts in place; a completed match starts a new session with that name
("again"); otherwise a new named session is created and started. While
another session runs, a different named target swaps in. Empty `<X>` keeps
the running ledger. Use `#` when a number comes first (`=3#bugs`), the name
starts with a digit or reads as a close (`=#2fa`, `=#x2`), or the name is quoted.

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

Close today's running timed Pomodoro. Lowercase `x` only. Selection pieces
`*`, `!`, `~`, and `@` may appear in any order. `<N>` keeps those numbered
Task Links in progress; a lone `0` means none. `@<N>` is the same keep-list
typed last (`x!1@2` means `x2!1`).

```bash
bob capture --dry-run 'x'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] → [/] Write an outline work.md ^outline
  next: WORK (created) at line 10 · carries 1 link
```

A mid-body `x` stays prose (`Plan x`). `xx`, `x-ray`, and capital `X` never
close.

Contract: [Closing the running Pomodoro](../capture.md#closing-the-running-pomodoro).

### `x!<M>` {#x-complete-links}

Complete those numbered Task Links. A present-but-empty `!` completes every
remaining numbered link (`x!`). `x!1` narrows to task 1.

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

Park those links (normal work without carrying forward). Empty `*` parks
every remaining numbered link (`x*`).

```bash
bob capture --dry-run 'x*'
```

```text { .no-copy title="Output" }
[dry-run] ok would close WORK 1200-1225 → 1200-1200 (0m, −25m) · 2026/20261020.md line 8
  1 [*] → [ ] Write an outline work.md ^outline
```

Contract: [Choosing each Task Link's outcome](../capture.md#choosing-each-task-links-outcome).

### `x~<K>` {#x-drop-links}

Drop those links (removed, not carried, not started). `x~` is incomplete
(`pomodoro_close_task`).

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

One entry may sit on the close line; several use child bullets. With no
number, the line logs to the first task the close works. A leading number
names the task (`x 1 3 bugs fixed` logs `3 bugs fixed` to task 1). `x 1`
without text is incomplete (`pomodoro_close_log_text`). Bullets are numbered
all or none. A two-space bullet nests an undated detail.

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
like blank-line items (`+2 x`, `x =`, `x =bugs`, `=bugs +2`). An inline
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

### `@route^block-id+` {#project-plus}

Create the project note `<route>_<block_id>.md` (hyphens in the block ID
become underscores). The `^prj` task is never linked and the daily note is
untouched. `#pomodoro` on this form sends ` :id` Task Links under that named
open Pomodoro.

```bash
bob capture '@work^intake-flow+' 'Ship the intake flow'
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

### `:id` child {#project-colon-child}

On a project-note capture, `- <task> :<task-id>` names the task, makes it
Next (`[*]`, or `[?]` when scheduled), and links `[[<stem>#^<task-id>]]`
into the current/next Pomodoro, or the `#pomodoro` one.

```bash
bob capture '@work^intake-flow+' 'Ship the intake flow' '- Name the stages :stages'
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

Ordinary child bullet under an existing task.

```bash
bob capture 'Check the parser @work+prep'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  under [ ] Prep the intake  ^prep
  - Check the parser
```

Contract: [Sub-bullets under existing tasks](../capture.md#sub-bullets-under-existing-tasks).

### `@route+block-id#section` {#plus-child-section}

Child bullet under an ALL-CAPS section of that task. With no other text, `#`
names a Pomodoro instead (Ensure Next / move).

```bash
bob capture 'Need a parser @work+prep#requirements'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  under [ ] Prep the intake  ^prep  REQUIREMENTS
  - Need a parser
```

Contract: [Sub-bullets under existing tasks](../capture.md#sub-bullets-under-existing-tasks).

### `@route+block-id` with no other text {#plus-ensure-next}

`@route+block-id` with no other text ensures the task is Next and relocates
its existing open-Pomodoro Task Link to today's implicit current/next
Pomodoro. `#pomodoro` moves the subtree to that named open Pomodoro,
creating a named future Pomodoro when missing. Own-prerequisite Blocked
targets drop those links first.

```bash
bob capture '@work+outline'
```

```text { .no-copy title="Output" }
✓ ensure-next  work.md
  [ ] → [*]  Write an outline  ^outline
  moved Task Link under WORK
```

```bob-task
- [*] #task Write an outline [created::2026-10-20] ^outline
```

Contract: [Task Link toggle](../capture.md#task-link-toggle).

### `@route+block-id!` {#plus-toggle}

Toggle that task's Pomodoro Task Link: link it when unlinked (Ready and
own-prerequisite Blocked rise to Next; other Blocked stay unlinked) or
unlink it when linked (lane unchanged). Removing a link never lowers Next
or Pending.

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

### trailing `#` {#trailing-hash}

A trailing bare `#` is a plain-text note on a Pomodoro, not a routed task.

```bash
bob capture 'remembered the timeout #'
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

### `%` / `%N` / `%header` {#percent-clipboard}

Capture clipboard content as child bullets. `%`, `%N`, and `%header` are
the spellings. `-n, --no-clip` keeps trailing `%` literal. `-c, --clip`
forces clipboard capture. Clipboard backends are stubbed in this isolated
fixture, so the example keeps `%` literal.

```bash
bob capture --no-clip 'Keep the percent @work %'
```

```text { .no-copy title="Output" }
✓ captured  work.md
  - [ ] #task Keep the percent % [created::2026-10-20]
```

Contract: [Clipboard](../capture.md#clipboard).

### `t:<type>` {#t-type}

Type the new task at either line edge (`t:fix Fix it`, `Fix it t:fix`).
Lowercase only; `T:` stays prose. One marker per item. Slugs use letters,
digits, and hyphens; quoted literals allow spaces (`t:"DEEP WORK"`).
Unknown names write canonical spelling with a warning. Existing targets
are refused until a later phase. With `#NAME`, the marker must agree with
the Pomodoro or the capture refuses with `type_marker_conflict`.

```bash
bob capture --no-clip 'Fix the parser t:fix @sase'
```

```text { .no-copy title="Output" }
✓ captured  sase.md
  - [ ] #task Fix the parser [type:: FIX] [created::2026-10-20]
  type   → FIX · from sase suggests WRITE
```

Contract: [Typing tasks with t:](../capture.md#typing-tasks-with-t).

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Capture contract](../capture.md)
- [Vault format](vault-format.md)
- [bob capture CLI](cli/capture.md)
- [Capture](glossary.md#capture) and [Bulk Capture](glossary.md#bulk-capture)
- [Task dependency links](../task-dependencies.md)
- [Plan budget, Today, and lanes](../plan.md)
