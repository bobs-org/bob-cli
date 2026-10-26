---
title: Vault format
description: Task-line anatomy, lanes, fields, and the note types that make up a Bob vault.
---

# Vault format

A Bob vault is an Obsidian folder of Markdown notes. Tasks live as `#task`
checkbox lines in area and project notes. Today is read from the Pomodoro
ledger. This page is the lookup for the stored forms. Behavior contracts own
the rules.

Examples use a synthetic vault and `BOB_NOW=2026-10-20`. Default paths are
relative to `BOB_DIR` (`~/bob` unless you set it or pass `--bob-dir`). Copy
the bundled demo with the [first Bob day tutorial](../start/first-bob-day.md)
when you need that fixture.

## Task-line anatomy

Rendered line (canonical Markdown has no callout markers):

```bob-task
- [?] #task Review the intake queue [fresh:: 2026-10-13] [refresh:: 7] [keeps:: 2] [unblocked:: 2026-10-12] [scheduled:: 2026-10-20] [created:: 2026-10-01] [id:: work__queue] [dependsOn:: work__prep] ^queue
```

| # | Token | Meaning |
| --- | --- | --- |
| 1 | `- [?]` | List marker and Blocked checkbox. The symbol is the lane; Blocked is derived. |
| 2 | `#task` | Tracked-task identity. Exact `#task` is required. Display may show a teal hash glyph. |
| 3 | `Review the intake queue` | Description. Wikilinks and other tags may follow. |
| 4 | `[fresh:: 2026-10-13]` | Last human confirmation date. Absence means never confirmed (NEW). |
| 5 | `[refresh:: 7]` | Per-task review interval in days. |
| 6 | `[keeps:: 2]` | Keep streak on due Ready reviews. Absence means 0. |
| 7 | `[unblocked:: 2026-10-12]` | Date of an automatic Blocked → Ready recovery. |
| 8 | `[scheduled:: 2026-10-20]` | Tasks scheduled date. Future dates keep the task Blocked. |
| 9 | `[created:: 2026-10-01]` | Capture date. Written by capture; not a freshness stamp. |
| 10 | `[id:: work__queue]` | Tasks id, derived from the note path and block ID when Bob mints it. |
| 11 | `[dependsOn:: work__prep]` | Derived projection of the Depends-On line. |
| 12 | `^queue` | Trailing block ID. Task Links and capture tokens point here. |

A dedicated Task Link under today's open Pomodoros adds a display-only 🍅
Today mark. The source line never stores 🍅 or a Today field.

The freshness group (`fresh`, `refresh`, `keeps`, `unblocked`) sits before
the trailing Tasks suffix (`scheduled`, `created`, `id`, `dependsOn`). The
block ID is last.

Verified capture of a new blocked dependent (spacing is what `bob capture`
wrote):

```bob-task
- [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
```

Contract: [Task freshness fields](../freshness.md#2-fields-overrides-and-config),
[Task date marks](../date-marks.md),
[Task tag marks](../task-tag-marks.md),
[Today marks](../today-marks.md).

## Checkboxes and lanes

| Checkbox | Glyph | Lane name | Tasks type | Who writes it |
| --- | --- | --- | --- | --- |
| `[ ]` | ○ | Ready | `TODO` | Default for a new unscheduled task. Alt+N on Next/Pending returns here. |
| `[*]` | ● | Next | `ON_HOLD` | Linking a Ready task under today's open Pomodoros, or Alt+N commit. Sticky. |
| `[/]` | ◑ | In Progress, also called Pending or WIP | `IN_PROGRESS` | An `=x` in-progress close outcome, or Obsidian Alt+[ / Alt+]. Sticky. Dash and chip labels say Pending. |
| `[?]` | ? | Blocked | `ON_HOLD` | Derived: open Depends-On targets, a future `scheduled` date, or a blocked project. |
| `[x]` / `[X]` | ✓ | Done | `DONE` | Completion (`!note:id`, `x!`, Obsidian Done). |
| `[-]` | ⊘ | Cancelled | `CANCELLED` | Cancel gesture. |

Next and Pending do not fall back to Ready when their Task Links disappear.
Only an explicit release (Alt+N, or the Task Card lane row) lowers them.
Blocked overrides every lane while its reason holds, then recovery returns
the task to its derived rank.

Configure Obsidian Tasks to the **Dataview** format and the `#task` global
filter, with a custom Blocked status whose symbol is `?`. Without
`.obsidian/plugins/obsidian-tasks-plugin/data.json`, native Tasks queries
use emoji format and several writers refuse Blocked transitions.

Contract: [Getting started](../getting-started.md#vault-requirements),
[Task Status Hooks](../task-status-hooks.md),
[Next and Pending are sticky](../develop/decisions.md#task-lanes-are-sticky).

## Authored and derived fields

Edit the human-owned inputs. Reconciliation and capture project the rest.

| Field | Form | Ownership |
| --- | --- | --- |
| `scheduled` | `[scheduled:: YYYY-MM-DD]` | Authored. Capture `s:<N>` and `$<N>`, Task Card, and `bob task reroll` write it. A date after today blocks the task. |
| `due` | `[due:: YYYY-MM-DD]` | Authored Tasks field. Bob almost never writes it; date marks do not render it. |
| `created` | `[created::YYYY-MM-DD]` | Written on capture (local date from `BOB_NOW`, `DATE`, or the clock). Not a confirmation stamp. |
| `completion` | `[completion:: YYYY-MM-DD]` | Written when the task is Done. |
| `cancelled` | `[cancelled:: YYYY-MM-DD]` | Written when the task is Cancelled. |
| `priority` | `[priority:: high]` (and the configured values) | Authored by `$<N>` / Task Card. A missing field is implicit P0. |
| `id` | `[id:: note__blockid]` | Derived. Minted when a dependency writer needs one. Do not treat a hand edit as the identity source. |
| `dependsOn` | `[dependsOn:: a, b]` | Derived from the Depends-On line. |
| `fresh` | `[fresh:: YYYY-MM-DD]` | Human confirmation only. Creation and automation never stamp it. |
| `refresh` | `[refresh:: N]` | Optional per-task interval, 1–365 days. |
| `keeps` | `[keeps:: N]` | Human keep streak, 1–999. Automation never increments it. |
| `unblocked` | `[unblocked:: YYYY-MM-DD]` | Machine WRITE on automatic Blocked → Ready recovery, unless a skip applies. |
| `repeat` | `[repeat:: every day when done]` | Authored Tasks recurrence rule that must start with `every`; PRE/POST checklist rows need `when done`; rendered as a [recurrence mark](../date-marks.md#recurrence-not-a-date). |

`start` on a task line is out of scope for date marks; Pomodoro entries use
`[t:: 15m]` on the session row, not on the task.

Contract: [Date marks fields](../date-marks.md#fields-and-glyphs),
[Freshness fields](../freshness.md#2-fields-overrides-and-config),
[Dependency field placement](../task-dependencies.md#3-identity-and-link-form).

## Depends-On line

A task's prerequisites live on one managed first-child line. That line is
the source of truth.

```markdown
- [?] #task Review the intake queue [created::2026-10-20] [id:: work__queue] [dependsOn:: work__prep] ^queue
	- ⛓️ **DEPENDS ON:** [[#^prep]]
```

The prerequisite's generated mirror (never read back as input):

```markdown
- [ ] #task Prep the intake [created::2026-10-20] [id:: work__prep] ^prep
	- 🧱 **BLOCKS:** [[#^queue]]
```

Writer form: `⛓️` (U+26D3 U+FE0F), space, `**DEPENDS ON:**`, space, then
plain Task Links joined by ` • `. No aliases, embeds, or strikes. An empty
list deletes the line and the field.

A task's directed relations live on one authored line, with a generated
mirror maintained by reconcile:

```markdown
- [ ] #task Fix flaky capture retries ^fix-retries
	- 🪢 **LINKS:** follow-up of [[done/bob_done#^capture-star]] • [[sase#^retry-design]]
- [ ] #task Design a retry policy ^retry-design
	- 🪢 **LINKED FROM:** [[#^fix-retries]] (follow-up of)
```

Writer form: `🪢` (U+1FAA2, no VS16), space, `**LINKS:**` or
`**LINKED FROM:**`, space, then entries joined by ` • `. A LINKS entry
is an optional label plus a block link to a task
(`[<label> ][[note#^id]]`); a LINKED FROM entry appends the label in
parentheses (`[[note#^id]] (<label>)`). An empty list deletes the line.
Relations never block, count as Pomodoro Task Links, or change status.

Other machine-owned direct children (closed registry): `❌ **CANCEL LOG**`,
`🧱 **BLOCKS:**`, `🧩 **Sub-projects:**` on `^prj`,
`🪢 **LINKS:**`, `🪢 **LINKED FROM:**`, `🗓️ **SCHEDULE LOG**`,
`🛠️ **WORK LOG**`, `🥀 **ROTTEN LOG**` with newest-first
`*YYYY-MM-DD* — <reason> · kept N×` entries. Writers emit `🥀 **ROTTEN LOG**`
(U+1F940, no VS16); parsers also accept an optional U+FE0F, the emoji-less
`**ROTTEN LOG**`, and the legacy `**Rotten log**`, each with an optional colon
and only trailing spaces or tabs. None of these is a Pomodoro Task Link.

Contract: [Task dependency links](../task-dependencies.md).

## Lane, freshness, and Today

These three readings share a task line and mean different things.

| Reading | What it is | Stored as |
| --- | --- | --- |
| Lane | Ready / Next / Pending / Blocked / Done / Cancelled | Checkbox symbol. Next and Pending are sticky. Blocked is derived. |
| Freshness | Whether a human has confirmed the task as written, and whether that confirmation is still due | `[fresh::]`, `[refresh::]`, `[keeps::]`, `[unblocked::]`. Never a lane change. |
| Today | Open tasks with a dedicated Task Link under today's **open** Pomodoros | Computed at read time. Not a tag, field, or path filter. |

The morning walk visits PRE → NEW → PROJECTS → PENDING → NEXT → TICKLER →
REFERENCES → ROTTEN → POST (plus RECURRING when those tasks are due). Tiers
are a review queue, not extra checkboxes.

A dedicated Task Link is a direct child bullet of an open Pomodoro whose
body, after stripping 🍅 markers, is exactly one `[[target#^id]]` or
`![[target#^id]]`. Deeper descendants, mixed prose, Depends-On lines, and
Work Log lines do not count.

Contract: [Today](../plan.md#today),
[Lanes](../plan.md#lanes-next-and-pending),
[Task freshness](../freshness.md),
[Review walk](../develop/decisions.md#review-walk-is-tiered).

## Pending {#pending}

Pending is the whole `[/]` (In Progress) lane: not done, not
dependency-blocked, not `#hide`, not under `_templates` or `_conflicts`, and
with no scheduled date after today. The dashboard Pending *section* excludes
Today and `dash.md`; `bob plan` counts the whole lane, Today included.

The cap is 10 by default (`plan.max_pending`). Crossing it lints; it never
refuses a capture. Daily review uses `freshness.pending_interval` (default
1 day; `false` turns that lane walk off).

Release (Alt+N) returns Pending to Ready and offers an optional Work Log
summary. Unlinking without release leaves the task Pending.

Contract: [Lanes (NEXT and PENDING)](../plan.md#lanes-next-and-pending).

## Next {#next}

Next is the whole `[*]` lane under the same visibility rules as Pending.
The cap is 15 by default (`plan.max_next`). Linking a Ready task under
today's open Pomodoros raises it to Next. Alt+N on a Ready task commits it
to Next without linking it.

A Blocked task keeps no Task Link under today's open Pomodoros. Capture's
existing-task link on a task Blocked only by its own prerequisites drops
those links, then performs the gesture. Project-blocked residents stay
Blocked.

Contract: [Lanes (NEXT and PENDING)](../plan.md#lanes-next-and-pending),
[Capture link drop](../develop/decisions.md#capture-link-drops-blocking-deps).

## Note types

Residence is the file that holds the checkbox line. A Task Link or embed
elsewhere never moves the task.

### Area notes

Frontmatter `type: "[[area]]"`. An ongoing sphere of responsibility with no
end state and no `^prj` task. `parent` names the area it sits under
(`"[[area]]"` for a top-level area). Root-level area notes are `@route`
capture targets.

```yaml
---
type: "[[area]]"
title: Work
parent: "[[area]]"
---
```

Optional `ready_cap: N` or `ready_cap: off` overrides
`plan.max_ready_per_note` (default 5) for that note's whole Ready lane.
Optional `task_refresh: N` overrides the Ready freshness interval for tasks
that live here. Optional `project_name_aliases` lists extra capture/ref
names for the stem.

Inbox files are area notes whose `parent` resolves to `inbox.md`
(`mac_inbox.md`, `gkeep_inbox.md` today), plus `inbox.md` itself.

Contract: [Area Note](glossary.md#area-note), [Inbox File](glossary.md#inbox-file).

### Project notes

Frontmatter `type: "[[project]]"`. The `^prj` task states the completion
criteria. Capture `@route^block-id+` creates
`<route>_<block_id>.md` (hyphens in the id become underscores).

Verified create (`bob capture '@work^intake-flow+' 'Ship the intake flow'`):

```yaml
---
parent: "[[work]]"
template: "[[new_project]]"
type: "[[project]]"
status: wip
created: 2026-10-20T12:00:00+0000
---
```

```bob-task
- [ ] #task #prj Ship the intake flow #hide ^prj
```

While `^prj` is Blocked by its own open prerequisites or a future inline
`scheduled` date, every open resident is Blocked and tagged `#prj_is_blocked`.
`^prj` itself is never tagged. `bob projects sync` maps checked to
`status: done`, canceled to `status: canceled`, and a reopened terminal
project to `status: wip`. A project note may set optional `root: true` to
become its own root parent for Stats grouping; see
[R2 — Root parent](../pomodoro-homes.md#r2-root-parent).

Contract: [Project Task Sync](../projects.md),
[Project Note](glossary.md#project-note),
[Project Task](glossary.md#project-task).

### Inbox notes

`inbox.md` at the vault root, plus area notes whose frontmatter `parent`
resolves directly to it. This is a note relationship, not a folder hierarchy;
projects and untyped children do not count. Default capture
target is `mac_inbox.md`. `bob gkeep pull` drains into `gkeep_inbox.md`
(`gkeep.target` can name another vault-relative note). Current inbox files
set `ready_cap: off`. New captures here have no `fresh` stamp, so they
surface in NEW.

Ordinary capture does not create a `## Tasks` heading or an area/project
type. Missing target notes are created when needed.

Contract: [Inbox File](glossary.md#inbox-file).

### Day notes and the Pomodoro ledger

Path `<bob-dir>/YYYY/YYYYMMDD.md` for Bob's current date, selected by the
configured `timezone` or the host timezone, or `BOB_DAY_FILE` when set.
The ledger is the `## Pomodoros…` section up to the next `## `
heading.

```markdown
---
parent: "[[day]]"
---
# Daily note

## Pomodoros

- [ ] (**1200-1215** [t:: 15m]) — WORK
	- [[work#^outline]]
- [ ] () — LATER
```

Column-0 checkbox items are Pomodoro entries. A timespan is a timed
session; `()` is a placeholder. An optional ` — NAME` suffix (em dash plus
ALL-CAPS name) names it. Child bullets that are exactly one Task Link are
that session's queued work. `bob capture` session operators (`=`, `+N`,
`++N`, `x`) rewrite this section.

A `+` separates themes within a name: `WORK + ADMIN` has two themes. The
theme catalog comes from canonical daily notes, with hide and color
preferences in `.bob/themes/<host>.jsonl`. Hiding affects Recent suggestions;
the ledger and its history remain intact.

Each daily note also templates a `^gtd` task for review chores. Canonical
daily-note Next tasks are the R9 carve-out from sticky lanes.

Contract: [The ledger](../plan.md#the-ledger), [Pomodoro](glossary.md#pomodoro),
[Pomodoro homes](../pomodoro-homes.md), [Pomodoro themes](../pomodoro-themes.md).

### Reference notes

Generated under `ref/` with `type: "[[ref]]"`. Nested library PDFs also
carry `ref_type` from `lib/<ref_type>/`. Pipeline fields
(`source_pdf`, hashes, highlight counts) are command-managed. The reading
task is `#task #ref` with trailing `^ref` (v1, in-note) or a v2 task in the
parent area/project/inbox that links the PDF.

```yaml
---
type: "[[ref]]"
parent: "[[work]]"
title: Example paper
ref_type: papers
status: ready
source_pdf: "[[lib/papers/example.pdf]]"
---
```

```bob-task
- [ ] #task #ref [[lib/papers/example.pdf]] #hide ^ref
```

Lookup and filing live in `bob ref`. Bare URL capture queues a job; its worker
creates the PDF, then attempts intake by default: filing the PDF in `lib/`
and creating its note in `ref/`. The note appears in
`bob ref list` after successful intake. `bob ref scan --intake-only` retries
deferred intake; a full `bob ref scan` also synchronizes annotations.

The `<!-- highlights:begin -->` / `<!-- highlights:end -->` region is
generated; keep authored notes outside it. Highlight quotes store underline
emphasis as `**…**` and use `^h-…` block IDs. Reordering distinct exported
annotations preserves those IDs. Removed annotations leave tombstones
("Removed highlight" blocks retaining the old IDs). A highlight already in
the note that joins a continuation leaves a redirect to the combined quote.
An underline absorbed into another highlight leaves a redirect only when a
vault Markdown link targets its old ID or a stub already exists.

Annotation sections are derived from Highlights comments and sticky notes:
`# Name` on the first nonempty line declares a section; a matching `@name`
or `#name` at the start or end of a non-task line joins it. Section headings
and summary cards render inside the managed region, with `^s-…` section
anchors and stable `^h-…` annotation IDs.
Edits to the source annotations in Highlights update the grouping on the next
full scan. See [Annotation sections](../highlights-ref-sync.md#sections-and).

Contract: [Reference library](../ref.md),
[Highlights Reference Note Sync](../highlights-ref-sync.md),
[Reference Note](glossary.md#reference-note).

## Directory tree

Default layout relative to `BOB_DIR`. Only some paths are required; others
appear when a command writes them.

```text
vault/
├── inbox.md                 # triage root (counts as an inbox file)
├── mac_inbox.md             # default capture target (area / inbox)
├── gkeep_inbox.md           # default bob gkeep pull target
├── work.md                  # area note; @work
├── work_intake_flow.md      # project note from @work^intake-flow+
├── 2026/20261020.md         # daily note / Pomodoro ledger
├── dash.md                  # dashboard (not a capture route)
├── done/                    # bob task archive
├── ref/                     # generated reference notes
│   └── papers/example.md
├── lib/                     # library PDFs (configurable)
│   └── papers/example.pdf
├── xlib/                    # host-local intake queue
├── img/                     # images written by bob capture
├── file/                    # saved clipboard snippets
├── _conflicts/              # local copies vault-sync could not merge
├── _templates/              # skipped by lane queries
├── .bob/                    # machine data; never scanned as notes
│   ├── themes/              # synced per-host theme preference journals
│   └── relations/           # synced per-host relation-label preference journals
└── .obsidian/plugins/       # installed community plugins, including Bob's
```

| Path | Required? | Override |
| --- | --- | --- |
| Vault root | Yes, an existing directory | `BOB_DIR`, `--bob-dir` |
| `YYYY/YYYYMMDD.md` | Needed for session capture and Today | `BOB_DAY_FILE` |
| `mac_inbox.md` | Created on first default capture | `@route`, `@@route`, `--route` |
| `ref/`, `lib/`, `xlib/` | Needed for the reference library | `bob ref` configuration |
| `gkeep_inbox.md` | Needed for Keep drain | `gkeep.target` |
| `done/` | Created by archive | none |
| `.obsidian/plugins/obsidian-tasks-plugin/data.json` | Needed for Dataview Tasks and Blocked writes | none |

`.bob/` is machine data (preference journals, Keep import receipts, and
saved reference sources). Note walkers never scan it as vault notes:
task archive, link repair, project and ready scans, task-status hooks,
`bob query`, and the gkeep ledger skip it.

`old_lib/` is the archival predecessor of `lib/`. Capture routes select
root-level notes: `@work` writes `work.md`. Nested notes are reached with
path-aware `&` / `*` (`&projects/foo:bar`), not as `@` routes.

Contract: [Reference library default paths](../highlights-ref-sync.md#default-paths).

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Capture syntax](capture-syntax.md)
- [Plan budget, Today, and lanes](../plan.md)
- [Task freshness](../freshness.md)
- [Task dependency links](../task-dependencies.md)
- [Task Status Hooks](../task-status-hooks.md)
- [Project Task Sync](../projects.md)
- [Today marks](../today-marks.md)
- [Date marks](../date-marks.md)
- [Pomodoro homes](../pomodoro-homes.md)
- [Reference library](../ref.md)
