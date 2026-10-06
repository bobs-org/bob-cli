# Project Task Sync

`bob projects` manages Bob project notes through one completion-criteria task
anchored with `^prj`. Typical CLI order:

1. `bob projects list` — inspect the vault
2. `bob projects sync --dry-run` — preview reconciliation
3. `bob projects sync` — apply it
4. `bob task reconcile` (formerly `bob task-status-hooks`, still accepted)
   — derive `[?]` Blocked markers for any schedules just written

`sync` writes frontmatter, `#hide`, Sub-projects lines, and inline
`[scheduled::]` fields. It does not change checkboxes. The Bob Navigation
Hotkeys Task Card in Obsidian can write schedules or priorities and reconcile
Blocked in one editor transaction; those gestures are not part of the CLI
command. See [Task Card](#task-card) and
[Scheduling from the `^prj` task](#scheduling-from-the-prj-task).

This mirrors the `bob highlights` `^ref` convention for `[[ref]]` notes: the
task line is the interaction point, and the command reconciles frontmatter from
that task instead of asking users to edit machine-facing metadata directly.

## Contents

- [Commands](#commands)
- [Project notes](#project-notes)
- [The `^prj` task](#the-prj-task)
- [Sync rules](#sync-rules)
- [Task Card](#task-card)
- [Scheduling from the `^prj` task](#scheduling-from-the-prj-task)
- [Priority property and scheduled rolls](#priority-property-and-scheduled-rolls)
- [Priority marks](#priority-marks)
- [Recommended roll and priority decay](#recommended-roll-and-priority-decay)
- [Schedule-log reason prompt](#schedule-log-reason-prompt)
- [Scheduling Work Log prompt](#scheduling-work-log-prompt)
- [Deferring a task prunes it from today's open Pomodoros](#deferring-a-task-prunes-it-from-todays-open-pomodoros)
- [Cancelling a task](#cancelling-a-task)
- [Inbox routing](#inbox-routing)
- [Warnings](#warnings)
- [Examples](#examples)

## Commands

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

`list` is read-only. It scans project notes, validates project scheduling,
prints frontmatter status, open `#task` count, dashboard-visible task count,
and the current `^prj` state.

`sync` mutates only the exact lines it needs to change. It prints one line for
each action or warning, then a summary. Per-file errors are reported without
stopping the rest of the scan, and the command exits 1 when any file error
occurred.

## Project Notes

A project note is any Markdown file in the vault whose frontmatter has:

```yaml
type: "[[project]]"
```

Bare `type: [[project]]` is accepted too. The scan skips `done/`, `.git/`,
`.obsidian/`, `_templates/`, and `_generated/`.

`sync` also reads an optional `parent` frontmatter field when it is an Obsidian
wikilink, such as `parent: "[[Parent Project]]"`.

Project scheduling is an optional frontmatter date:

```yaml
scheduled: 2026-07-16
```

Quoted values such as `scheduled: "2026-07-16"` are also accepted. The value
must be exactly `YYYY-MM-DD` and must be a real calendar date. Empty values,
timestamps, shortened dates, and impossible dates such as `2026-02-30` are
per-file scan errors for both `list` and `sync`. `sync` leaves a project with an
invalid schedule untouched and continues processing other project files.

## The `^prj` Task

Each active project should contain one task line like:

```markdown
- [ ] #task #prj Ship the project outcome! #hide ^prj
```

The trailing block id must be exactly `^prj`. The `#prj` tag immediately after
`#task` marks this as the machine-managed project lifecycle task so Obsidian
task views can tell it apart from ordinary follow-up tasks; it is additive, and
legacy lines without `#prj` are still recognized. Multiple `^prj` tasks or a
`^prj` line that is not a valid `#task` checkbox are per-file errors.

The same `^prj` line doubles as the project's freshness tracker: while
sync leaves it visible (no `#hide`), the morning review reminds Bryan
to replenish it on its tracker cadence (`project_interval` when set,
otherwise the normal Ready interval chain; see
[`docs/freshness.md`](freshness.md) §4, "Tracking review"), unless it
is also a PRE/POST checklist row. Review is gated by sync's `#hide`,
not a separate predicate.

Task statuses follow the Tasks plugin convention:

```text
[ ]  open
[/]  open
[*]  open
[x]  done
[X]  done
[-]  canceled
```

## Sync Rules

`bob projects sync` applies these rules:

- `[x]` or `[X]` on the `^prj` task sets frontmatter `status: done`.
- `[-]` on the `^prj` task sets frontmatter `status: canceled`.
- An open `^prj` task on a terminal project, `status: done` or
  `status: canceled`, reopens it to `status: wip`. Open `^prj` tasks on `wip`,
  `waiting`, or other non-terminal projects leave the status unchanged.
- Active projects with zero non-hidden open tasks and no open sub-projects
  have `#hide` removed from their open `^prj` task so they surface in
  `dash.md`'s Tasks section.
- Active projects with non-hidden open tasks or open sub-projects get
  `#hide` added back to their open `^prj` task immediately before `^prj`.
- A valid `scheduled: P` frontmatter date overrides surfacing only when
  it is in the future. Every ordinary task with an open marker (`[ ]`,
  `[*]`, `[/]`, or `[?]`) receives `[scheduled:: P]`, unless its one
  existing square-bracket or parenthesized `scheduled` field is a valid
  date equal to or later than `P`. Missing, malformed, and earlier
  values are written in the canonical square-bracket form. Emoji Tasks
  dates such as `⏳ 2026-08-01` are not inline Dataview fields and are
  not considered. On a today/past date — or with no `scheduled` at all
  — the normal surfacing rule below applies.
- Ordinary tasks lose all whole-token `#hide` tags at every checkbox status;
  near-matches such as `#hidden` and `#hideaway` remain. An ordinary task with
  multiple `scheduled` fields is left completely unchanged and reported as a
  non-fatal warning. Done, canceled, and unknown/custom-status tasks otherwise
  receive no schedule field.
- The `^prj` lifecycle task never receives an inline schedule. A future
  project date forces exactly one `#hide` on it. `#hide` therefore
  remains the lifecycle surfacing mechanism for `^prj`, not for
  scheduled ordinary tasks.
- Schedule propagation applies only to non-terminal projects. It preserves
  list markers, indentation, descriptions, other inline fields, trailing block
  IDs, CRLF line endings, and unrelated tags. Frontmatter, fenced examples,
  non-task lines, and checkbox-like prose are ignored. `BOB_NOW` overrides the
  local date boundary for deterministic previews and tests; repeated syncs are
  idempotent.

For a valid project date `P`, the ordinary-task contract is:

| Task state | Result |
| --- | --- |
| Open, absent/malformed schedule or valid schedule before `P` | Set `[scheduled:: P]`; remove `#hide` |
| Open, one valid schedule on/after `P` | Keep that schedule; remove `#hide` |
| Open, multiple `scheduled` fields | Leave the line unchanged; warn |
| Done, canceled, or unknown/custom | Remove `#hide` only |
| `^prj`, `P` in the future | Force exactly one `#hide`; never add an inline schedule |
| `^prj`, `P` due/past | Normal surfacing rule (rows above): remove `#hide` when there are no unhidden open tasks and no open sub-projects, add it back otherwise |
- Active projects with open `^prj` tasks get one generated Sub-projects line
  nested directly under `^prj`, such as
  `- 🧩 **Sub-projects:** [[alpha_child]] • [[beta_child]]`.
- A child with a valid `scheduled` frontmatter date later than the machine's
  local current date is prefixed with `🗓️`, such as
  `- 🧩 **Sub-projects:** 🗓️ [[future_child]] • [[ordinary_child]]`.
  Today, past, absent, and invalid schedules do not receive the marker.
  `BOB_NOW` controls this date boundary as it does for task schedules.
- The marker-prefixed Sub-projects line is fully machine-owned and rewritten
  into canonical form. Duplicate marker lines are removed. The line is deleted
  only when there are no open sub-projects and no tracked closed sub-projects
  left to show. Sync adds or removes `🗓️` as schedules change and removes it
  automatically on the scheduled date.
- Closed sub-projects already present on the generated line are retained as a
  ledger: done children render as `~~[[child]]~~ ✅`, and canceled children
  render as `~~[[child]]~~ ❌`. Schedule and lifecycle decorations are
  independent, so a retained future-scheduled done child renders as
  `🗓️ ~~[[child]]~~ ✅`.
- Every other sub-bullet under `^prj` is user-owned, including bare wikilinks
  like `- [[scratch_note]]`; `sync` never removes or uses them to suppress the
  generated line.
- Existing inline `scheduled` fields are removed from open `^prj` tasks on
  active projects. Frontmatter `scheduled` is the sole project schedule.
- Terminal projects, `status: done` or `status: canceled`, get no `^prj` line
  edits while their `^prj` task stays closed or missing. Reopening the `^prj`
  task makes the project active again in the same run, so the surfacing,
  `#hide`, and Sub-projects rules above apply from the reopened `wip` status.

`bob projects sync` writes task schedules but does not write checkbox markers.
`bob task reconcile` remains the single CLI owner of derived `[?]` Blocked
state: run it after `projects sync` to block future-scheduled tasks or recover
matured ones once no dependency or schedule reason remains. The dashboard
already excludes future schedules and Blocked tasks, so `dash.md` needs no
query change. These tasks now appear in `blocked.md`; that query also needs no
change. Derived Blocked status outranks Pomodoro promotion.

An open sub-project is another project note whose `parent` wikilink resolves to
this note's file stem and whose own `^prj` task is open. A child with terminal
frontmatter but an open `^prj` task counts as open in the same run, because the
open task reopens it to `wip`. Checked or canceled child projects do not keep the
parent hidden; missing, malformed, or multiple non-terminal `^prj` child tasks
are excluded from the generated line.

Generated sub-project links use the child note's file stem with its original
casing and no path or alias. Open children are always shown first, sorted
case-insensitively. Closed children that were already listed are shown after
open children, also sorted case-insensitively. Links are separated with `•` on
the single marker-prefixed line.

Closed children are preserve-and-mark only: `sync` marks a terminal child if it
is already on the generated line, but it does not resurrect older closed
children that are not listed. Deleting a closed entry by hand prunes it
permanently unless that child is reopened.

In `bob projects list`, the `SHOWN` column counts open, non-`^prj`, non-hidden
`#task` lines that are neither `[?]` nor validly scheduled later than the local
current date. The separate open non-hidden count still drives `^prj` surfacing,
so dependency-blocked tasks do not accidentally surface the lifecycle task.
An open `^prj` task with a `#hide` tag renders as `open`; an open `^prj` task
without a `#hide` tag renders as `on dash`.

When a project has no `status:` line and the `^prj` task is checked or canceled,
`sync` inserts `status: done` or `status: canceled` immediately after the
`type:` line.

The Bob Navigation Hotkeys "Create project note from task" command
(`<ctrl+shift+option+n`) transfers a
valid `[scheduled:: YYYY-MM-DD]` source-task field into the new project's
frontmatter and removes it from the completion criteria. Invalid or duplicate
schedule fields stop creation with a focused notice. `bob capture` can create
the same kind of project note directly, with no Obsidian window open: see
[Project notes](capture.md#project-notes). Task bullets in that capture can
name their IDs with a trailing ` ^<id>`, or name them and link them into the
Pomodoro with a trailing ` :<id>`. In the `<ctrl+=>` child
note picker, future-scheduled projects show a `calendar-clock` chip immediately
before the status pill; the chip says `Tomorrow`, `Jul 16`, or `Jul 16, 2027`
while its tooltip and accessible label expose the full date. Today, past,
missing, and invalid dates do not receive a chip. The compact `🗓️` in the
generated parent ledger represents the same future-only state without the
picker's labeled date chip.

A direct child bullet under the source task becomes a project-note **section**
instead of a task when it has no checkbox, its trimmed body is an ALL-CAPS
title (letters, digits, spaces, and a small set of punctuation), and it has at
least one nested list item of its own; a bare ALL-CAPS bullet with no nested
list items, or one written with a checkbox, still converts to a task as
usual. A qualifying bullet's body is lowercased and then title-cased —
`FUTURE WORK` becomes `Future Work`, `NON-GOALS` becomes `Non-Goals` — without
preserving acronyms, so `API DESIGN` becomes `Api Design`. The title reuses a
matching `##` header already on the note, case-insensitively and without
touching the header's own casing, appending the notes after that section's
existing content; with no match, a new `## Title` section is appended at the
end of the note. Either way the bullet's descendants are copied in verbatim,
re-indented but otherwise untouched — no `#task` token and no `[created::]`
field — so they read as reference notes rather than tasks.

The same hotkey on a project note's `^prj` task restores that project as a
single task in the parent note's `## Tasks` section. Tasks, uppercased
section bullets, and the `^prj` task's own sub-bullets — including managed
schedule and work logs — come back as children; `scheduled` and `created`
return as inline fields, and a block ID is derived from the project note
name. Inbound `#^prj` links are repointed at the restored task and the
project note is moved to the trash. Content outside that expected format
fails with a notice and changes nothing.

A managed source-task log of either kind — schedule (`🗓️ **SCHEDULE LOG**`,
`**SCHEDULE LOG**`, or legacy `**Schedule log:**`) or work (`🛠️ **WORK LOG**`,
`**WORK LOG**`, or legacy `**Work log:**`) — moves with the source task
instead. It lands as a direct child of the new project's `^prj` task rather
than a new `## Tasks` task line, preserving its marker spelling and nested
entries, and keeping source order when both kinds are present. Later
`Ctrl+Shift+P` and `Ctrl+Shift+Enter` edits on the `^prj` task continue
appending to the matching log.

### Task Card

`Ctrl+Shift+P` (`bob-navigation-hotkeys:set-bullet-property`, palette
**Task card (set properties)**) always opens the Task Card. There is no plugin
setting, no activation date, and no filtered property list: the card is the
only surface. A `taskCard` value left in the plugin's saved data by the old
setting is ignored. Direct Depends-On-line, chip, palette dependency, and
decay Less often entries still skip the card and open their stage. A commit
on the row the review walk just landed on advances the walk once to the next
remaining item.

| Gesture on the card | Outcome |
| --- | --- |
| `1`–`4` | Set that configured P-level and its frozen displayed date; extra configured levels get unique digits up to `9` |
| `0` | Clear priority to implicit P0; keep the scheduled date |
| `Ctrl+Enter` | Apply the cached recommendation (`Cmd+Enter` alias) |
| `Ctrl+R` | Regenerate recommendation and priority previews; no write |
| `Enter` | Open the selected action; Schedule is the default |
| `b` | Blocked by (the existing Depends on stage) |
| `f` | Review every (the existing Refresh stage) |
| `x` | Open the cancel-reason stage; never cancel on the key alone |
| `Alt+N` | Commit to Next or release to Ready |
| `Ctrl+D` | Delete the selected property (default selection is Schedule) |
| Backspace on empty / Back | Return to the card without writing |
| Escape, `q`, `Q` | Close and discard uncommitted state; `q` never closes while a text field is focused |
| `Ctrl+[` | Close from the card or any stage it opened, even from a focused date, reason, or Work summary field; nothing is written |

Any card gesture that writes closes the card. On a Next or Pending task, a
P-level or recommendation gesture first opens the Work summary stage (see
[Scheduling Work Log prompt](#scheduling-work-log-prompt)).

Same-level `2` on P2 is a deliberate re-pick that resets the roll streak.
`Ctrl+Enter` remains a roll that can advance the decay ladder. Custom list
properties stay in the More section; a More row opens that property's value
stage. Counted sessions name the N+1 scope and mixed values; Task Links say
`via Task Link` and name target notes. Cross-note Task Link writes do not undo
from the daily note with one Ctrl+Z.

Two visible differences from the old first screen: bare Enter on an
unprioritized task opens Schedule instead of committing the lane, and
`Ctrl+D` with default focus clears Schedule. Alt+N remains the fastest lane
gesture. Unbound keys do nothing: letters, `/`, and digits that do not map to
a configured P-level never open a list and never write.

**Scheduling input.** On the card's Schedule stage the date field accepts a
bare `N` days (`0` today, `1` tomorrow);
unsigned `Nd`, `Nw`, and `Nm`; weekday names `mon`…`sun` (the next occurrence
strictly after today); and the existing ISO, `M/D`, `M-D`, `+Nd/w/m`, and preset
forms. The preview row shows the weekday, ISO date, relative distance, and the
year at a year rollover. A complete date token followed by whitespace and text
is an inline reason (`3 waiting on API`). `Shift+Enter` skips the reason via
the blank-reason rule without skipping an applicable Work summary; otherwise
one combined Reason/Work summary review opens (see
[Schedule-log reason prompt](#schedule-log-reason-prompt)). Invalid, negative,
overflow, or ambiguous input never writes. Other date properties reached from
More use the strict parser: ISO, `M/D`, `M-D`, `+Nd/w/m`, and preset forms.

### Scheduling from the `^prj` task

With the cursor on a valid `#task ... ^prj` lifecycle task, Bob Navigation
Hotkeys' `Ctrl+Shift+P` Task Card **Schedule** action treats
`scheduled` as a project-note property. Choosing a date writes canonical `scheduled: YYYY-MM-DD`
YAML, removes any stale inline `[scheduled:: ...]` field from `^prj`, and
immediately propagates task-level schedules. It also applies the derived status
decision in the same guarded editor transaction: future-scheduled tasks become
Blocked, while due tasks recover to a safely proven Ready, Next, or In Progress
rank. A later task-owned schedule is preserved and remains Blocked. When the
vault snapshot cannot prove recovery, the property edit proceeds and `[?]` is
left for `bob task reconcile`. Other picker properties remain inline Dataview fields on the task.
`dependsOn` is not one of them: it is derived from the task's Depends-On
line (`docs/task-dependencies.md`), never written by hand.

Pressing `Ctrl+D` on the project-backed `scheduled` item removes the YAML
property, removes inline schedules exactly equal to that project date from
ordinary open tasks, and reconciles their Blocked markers. Other task-owned
schedule values remain. The `^prj` `#hide` surfacing decision remains owned by
`bob projects sync`.

Removing or editing project frontmatter outside `Ctrl+D` cannot identify which
task fields were propagated, so those fields remain. Prefer `Ctrl+D` when
unscheduling a project.

On a `^prj` lifecycle task, a schedule-log reason (see below) is written under
the `^prj` task's own bullet, alongside the project's other child content. It
is plain Markdown, not a Dataview field, so `bob projects sync` never reads or
touches it.

### Priority property and scheduled rolls

The same `Ctrl+Shift+P` picker offers a `priority` property for ordinary tasks
and `^prj` lifecycle tasks. The picker shows P-level labels, but the Markdown
stores Obsidian Tasks' native priority names:

| Picker label | Written field         | Random `scheduled` window |
| ------------ | --------------------- | ------------------------- |
| `P1`         | `[priority:: high]`   | 2-7 days from today       |
| `P2`         | `[priority:: medium]` | 8-30 days from today      |
| `P3`         | `[priority:: low]`    | 31-90 days from today     |
| `P4`         | `[priority:: lowest]` | 91-365 days from today    |

The split is deliberate. With the vault's Dataview task format, `priority` is a
reserved Tasks key whose accepted values are `highest`, `high`, `medium`,
`low`, and `lowest`. Tasks parsers read trailing inline fields right-to-left and
stop at the first unrecognized field, so a literal `[priority:: P2]` at the end
of a task would also hide earlier `[scheduled:: ...]`, `[id:: ...]`, or
`[dependsOn:: ...]` fields from task queries and dependency handling.

A task with no priority field is implicit P0, the highest priority: do it now,
with no rolled date. The Task Card's `0` key clears the field and keeps the
scheduled date; there is no P0 row. Clearing priority does not remove or re-roll `scheduled`,
because the rolled date is treated as an explicit commitment once written.

Choosing P1, P2, P3, or P4 writes the priority and rolls a `scheduled` date
inside that level's configured window in one guarded edit. Counted sessions
(`N<Ctrl+Shift+P>`) apply the same priority to each selected task while rolling
an independent scheduled date per task, and each generated schedule-log reason
records that task's own selected offset. On a `^prj` lifecycle task, priority
stays inline and the rolled date goes to project frontmatter, matching ordinary
`scheduled` picker behavior for project notes.

The write also records a `🗓️ **SCHEDULE LOG**` entry naming the priority
transition, the exact selected relative day, and the configured roll window,
without prompting:

```markdown
- [?] #task Ship the thing [priority:: medium] [scheduled:: 2026-09-02] ^ship
  - 🗓️ **SCHEDULE LOG**
    - _2026-08-13 → 2026-09-02_ — 🎲 P1 → P2 · in **20** (8–30) days
```

The bold number is the actual day offset selected for the scheduled date. The
parenthesized range is the configured priority window, and both endpoints are
shown even when they are equal.

See [Schedule-log reason prompt](#schedule-log-reason-prompt) for the full
deterministic-reason rules.

`bob capture <text> p:<N>` writes the same `[priority:: ...]` field and rolls
a date from the same configured window from the command line, reading the
same `~/.config/bob/config.yml` levels as the picker. `N` is the P-level key
(1-4 today), so `p:2` matches pressing `Ctrl+Shift+P` then `2` on the Task
Card. Capture leaves the task's `[ ]` marker as written; `bob task reconcile`
is what later marks a future-scheduled task Blocked, not capture itself. A
rolled `p:<N>` also writes the same `🗓️ **SCHEDULE LOG**` entry the picker
would, always as a `P0 → <to>` transition since a brand-new capture
never has a previous priority field. Its JSON output keeps the same
`schedule_log` shape; only the rendered reason string and rendered line text
carry the bold exact offset. `p:<N> s:<N>` writes no entry, because the
explicit `s:<N>` wins the scheduled date and the roll never happens; see
[Schedule-log reason prompt](#schedule-log-reason-prompt).

After a priority write, the Obsidian notice shows the chosen P-level, the
`[priority:: ...]` field that landed, the rolled ISO date with weekday, and the
date's distance from today. Counted sessions show the rolled scheduled span and
relative span instead of a single date. The notice also includes chips for
status side effects such as Blocked marking, propagated project schedules,
removed `#hide` tags, recovered tasks, unchanged tasks, and ambiguous scheduled
fields.

When the `scheduled` date picker opens on a task that already has a configured
priority, it pins a priority roll suggestion above the normal date presets.
Press `Ctrl+R` in that date picker stage to re-roll the suggestion before
choosing it. In counted sessions, the suggestion appears only when every counted
task has the same configured priority. Choosing the suggestion writes
immediately with its own deterministic reason instead of prompting; see
[Schedule-log reason prompt](#schedule-log-reason-prompt).

### Priority marks

This section is the authoritative display contract for task priority
marks, in the style of `docs/freshness.md` §11. The JavaScript
mirror is `api.priorityMarks` (namespace v1, top-level api v3) in
bob-ledger-tools; its tests run the conformance vectors below
verbatim.

Every canonical `[priority:: …]` task field renders as one compact
signal-bar glyph that reads the priority at a glance, in Live
Preview, reading view, embeds, hover previews, Dataview task views,
and Tasks query results. The stored Markdown never changes, the
cursor reveals the raw field for editing, and broken priority fields
get a visible repair flag. The Task Card level strip and priority
notices reuse the mark through `api.priorityMarks` v1, so you learn
it where you pick a priority.

**Principles** (borrowed from the freshness mark). Display-only:
`[priority:: value]` stays the only stored form; nothing writes the
mark, and the Rust and Tasks semantics are unchanged. Shape carries
meaning; color whispers. Truthful or neutral: when the ladder config
is unknown, the tooltip omits P-labels instead of guessing.
Reversible: the cursor or a click reveals the raw field, source mode
shows raw text, a session toggle restores today's pills and emoji,
and without bob-ledger-tools the vault looks exactly as it does
today. One glyph definition shared by every surface.

**The glyph: a signal staircase.** Four rising, pill-ended bars sit
in a 16-unit box. Filled bars show the priority, and a faint track
shows the bars that are left — the same silhouette the nav priority
notice already uses through Lucide `signal-high`/`-medium`/`-low`/`-zero`
(4/3/2/1 filled positions for P1–P4), so the mapping is already
familiar. The fill drains as a task decays P1 → P4, echoing the
freshness lease ring that drains with age. The glyph is chosen by
**stored Tasks value**, not ladder position, so it stays robust
without config. Final geometry (viewBox `0 0 16 16`, black fills,
used as CSS masks): bars as `rect`s with `width 2.4` and `rx 1.2`
at `x 1.1 y 10 h 4`, `x 4.9 y 7 h 7`, `x 8.7 y 4 h 10`,
`x 12.5 y 1 h 13` (shared bottom at y 14); urgent is a rounded
square `x 1.5..14.5`, `y 1..14`, `rx 3.2`, with the `!` knocked out
(`fill-rule="evenodd"`: stem rect `x 7.05 y 3.2 w 1.9 h 5.6`, dot
circle `cx 8 cy 11.1 r 1.1`).

| Stored value | Default label | Glyph                          |
| ------------ | ------------- | ------------------------------ |
| `high`       | P1            | 4 of 4 bars filled             |
| `medium`     | P2            | 3 of 4                         |
| `low`        | P3            | 2 of 4                         |
| `lowest`     | P4            | 1 of 4                         |
| `highest`    | (off-ladder)  | urgent: solid rounded square with a knocked-out `!`, no track, same footprint |
| no field     | P0            | nothing: no field to replace, and P0 is the absence of deferral |

**Tones (monochrome by design).** Filled bars use `--text-muted`;
the track is the same ink at 26% (the freshness track opacity).
Urgent uses `--text-normal`, so its weight carries the emphasis. On
closed tasks (`x`, `X`, `-`) the ink is `--text-faint` at opacity
0.75, derived purely in CSS from the nearest `li.task-list-item` /
`.HyperMD-task-line` `[data-task]` ancestor, so it works on every
surface; closed wins over any per-level color. Per-level theme hooks
`--bob-priority-color-{highest,high,medium,low,lowest}` are unset by
default: a user snippet can tint levels without touching the plugin.
Metrics match `.bob-fresh-mark`: interface font at 0.8em,
inline-flex, padding `0.06em 0.14em`, margin `0 0.08em`, pill
radius, `vertical-align: 0.05em`, opacity 0.85; hover is opacity 1
plus a 10% ink capsule; the glyph box is 1.08em square;
`[data-fold-space="true"]` adds `margin-inline-start: 0.3em`;
transitions are disabled under `prefers-reduced-motion`.

**One glyph definition: CSS masks.** `styles.css` defines the SVG
data-URI custom properties `--bob-priority-glyph-track`,
`--bob-priority-glyph-fill-1` … `-4`, and
`--bob-priority-glyph-urgent` once, on `body`. A glyph host draws
the track with `::before` and the fill with `::after`, both
absolutely positioned, using `-webkit-mask-image` and `mask-image`
with `background-color` from the ink variable. There are two kinds
of glyph host: `.bob-priority-mark[data-priority="…"]
.bob-priority-mark-glyph`, emitted by the plugin's JS; and
`body.bob-priority-marks .plugin-tasks-list-item
.task-priority[data-task-priority="…"]`, which is CSS-only for Tasks
results — the inner emoji span is visually hidden (clip pattern, not
`display: none`) so screen readers keep it, and the host gets the
glyph box plus `margin-inline-start: 0.3em`. No JS touches Tasks'
DOM, so re-renders, sorting, and `hide priority` keep working.

**Eligibility.** A canonical field matches
`^[\[(] *priority:: *(highest|high|medium|low|lowest) *[\])]`: the
brackets must be a matching pair, the key exactly `priority`, the
value lowercase. In Live Preview the line must be a task line
(quote-aware), hold exactly one `/priority\s*::/gi` occurrence, and
not sit inside code; in rendered views a text node must hold exactly
one canonical occurrence, sit under an `li.task-list-item`, and not
sit inside `code`, `pre`, `.dataview.inline-field`,
`.bob-priority-mark`, or `.bob-fresh-mark`. Anything else is left
alone (a Dataview pill or raw text).

**Repair flag.** While marks are on, any leftover `priority`
Dataview pill in Live Preview is non-canonical: uppercase values,
`P2`, `urgent`, duplicates, mismatched brackets, fields on non-task
lines. CSS flags it with full opacity and a dashed orange border,
matching the freshness repair flag, scoped to
`body.bob-priority-marks .markdown-source-view.is-live-preview`.

**Tooltip.** The mark carries `role="img"`, an `aria-label`, and
`data-tooltip-position="top"`, lines joined with `\n`, never
containing `::`. Value names are capitalized (`High`). On the
ladder: `P2 · Medium priority`, then `Rolls 8–30 days ahead`, then
`Ctrl+Shift+P to change` (equal bounds read `Rolls 5 days ahead`;
1–1 reads `Rolls 1 day ahead`). Off the ladder with the ladder
known: `Highest priority`, then `Not on the P1–P4 ladder` (first
and last ladder labels, or the single label for a one-level
ladder), then `Ctrl+Shift+P to change`. Ladder unknown (missing,
unreadable, or invalid config, no matching entry, or mobile):
`Medium priority`, then `Ctrl+Shift+P to change`.

**Surfaces and interaction.** Live Preview uses a `Prec.highest`
ViewPlugin emitting `Decoration.replace` with a widget; when the
character before the opening bracket is a space the range folds it
and the widget restores the gap, so the mark beats Dataview's pill
widget by position. The mark hides while any selection range
overlaps the field span, and a mousedown places the cursor at the
field start and focuses the editor — it never writes;
`Ctrl+Shift+P` stays the one way to change priority. Widget
equality compares a model key, so unchanged marks never flicker;
rebuilds happen on doc, viewport, or selection changes, Live Preview
or file switches, and a refresh effect. No marks in source mode.
The rendered-view post-processor (sort order 50, before Dataview's
inline-field pass at 100) splits the text node into before / mark /
after, covering reading view, embeds, hover previews, Dataview
`TASK` views, and Tasks descriptions that still carry a
non-trailing field. Tasks query results are CSS-only (no tooltip,
accepted; the `li` already carries `data-task-priority`).

**Session toggle.** The command "Toggle task priority marks" (id
`toggle-priority-marks`) is session-only and on by default. It flips
`body.bob-priority-marks` (which also gates the Tasks CSS and the
repair flag), dispatches the refresh effect to every markdown
editor, triggers the Tasks re-render event, and shows the Notice
`Priority marks on` / `Priority marks off`. When off, JS creates no
marks.

**Programmatic reuse.** `api.priorityMarks` (namespace v1,
top-level api stays v3) exposes `model(value)` and
`render(host, value, options)`: synchronous, never throwing;
`render` appends to `host` and returns the element, or `null` for
non-Tasks values. `options.decorative` renders `aria-hidden` with no
label for surfaces that already show a P-label;
`options.inheritColor` makes the ink `currentColor`.

**Rejected alternatives.** Changing storage to Tasks emoji or
P-codes breaks the Tasks Dataview format and rewrites bob-cli
parsers and writers, capture, and the Task Card, plus about 430
vault lines. A CSS-only restyle of the Dataview pill (like
`dependsOn`) cannot read the value text, so it cannot pick a glyph
per level. Lucide `signal-*` icons inline are stroke-only with no
track, hard to read at 0.8em, and P4 becomes a lone dot. A
per-level hue ramp clashes with the status colors while the existing
ramps already disagree. A P-label beside the glyph doubles the width
on every line; the label is one hover away and on the Task Card.
Marks on P0 tasks would glyph nearly every line with nothing to
replace. Click never opens the Task Card: display surfaces never
act. JS mutation of Tasks' DOM or a MutationObserver is fragile
across re-renders and costly; CSS on Tasks' own
`data-task-priority` is exact.

**Conformance vectors** (default ladder: P1 `high` 2–7, P2 `medium`
8–30, P3 `low` 31–90, P4 `lowest` 91–365; ⏎ separates tooltip lines):

| #    | Input                                                      | Expected                                                                               |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| PM1  | `- [ ] #task A [priority:: high] ^a`                       | 4 bars · `P1 · High priority⏎Rolls 2–7 days ahead⏎Ctrl+Shift+P to change` · fold space |
| PM2  | `- [?] #task B [priority::medium] [scheduled::2026-10-22]` | 3 bars · `P2 · Medium priority⏎Rolls 8–30 days ahead⏎…`                                |
| PM3  | `- [ ] #task C (priority:: low)`                           | 2 bars · P3, 31–90                                                                     |
| PM4  | `- [ ] #task D [ priority:: lowest ]`                      | 1 bar · P4, 91–365                                                                     |
| PM5  | `- [ ] #task E [priority:: highest]`                       | urgent · `Highest priority⏎Not on the P1–P4 ladder⏎Ctrl+Shift+P to change`             |
| PM6  | PM2 with the ladder unknown                                | 3 bars · `Medium priority⏎Ctrl+Shift+P to change`                                      |
| PM7  | `- [ ] #task F [priority:: High]`                          | no mark (repair pill)                                                                  |
| PM8  | `- [ ] #task G [priority:: P2]`                            | no mark                                                                                |
| PM9  | `- [ ] #task H [priority:: high] [priority:: low]`         | no mark on either                                                                      |
| PM10 | `- [ ] #task I [priority:: high)`                          | no mark                                                                                |
| PM11 | `- plain bullet [priority:: high]`                         | no mark                                                                                |
| PM12 | ``- [ ] #task J `[priority:: high]` ``                     | untouched (code)                                                                       |
| PM13 | ladder `{P1, highest, 1–3}`, `[priority:: highest]`        | urgent · `P1 · Highest priority⏎Rolls 1–3 days ahead⏎…`                                |
| PM14 | ladder `{P1, high, 5–5}` / `{P1, high, 1–1}`               | `Rolls 5 days ahead` / `Rolls 1 day ahead`                                             |
| PM15 | `- [x] #task K [priority:: high]`                          | 4 bars, resting tone (CSS)                                                             |
| PM16 | `> - [ ] #task L [priority:: medium]`                      | 3 bars (quote-aware)                                                                   |
| PM17 | `- [ ] #task M x[priority:: low]`                          | 2 bars, no fold space                                                                  |

**Live verification (Bryan, in Obsidian)** — still pending:

- P1–P4 and highest look right in light and dark themes
- the cursor or a click reveals the raw field
- `dash.md` Tasks results show glyphs, not emoji
- the Task Card shows the glyph beside P1–P4
- a hand-broken `[priority:: High]` shows the dashed repair pill
- the toggle restores the pills and emoji and then turns the marks back on
- closed tasks look resting
- fresh marks and priority marks sit together cleanly
- Metadata Menu does not double-decorate
- mobile (iOS) renders the glyphs

### Recommended roll and priority decay

`Ctrl+Enter` (`Cmd+Enter` on macOS) takes the recommended roll in one
keypress, and the date it will write is shown in the Task Card's
recommendation banner. `Ctrl+Shift+P` followed by `Ctrl+Enter` applies the
cached recommendation from the card's first screen with no navigation; the
same chord also works inside the Schedule stage. The roll follows a configurable
decay ladder read from the task's Schedule Log. A level is re-rolled `rolls` times
(default 1), the next recommended roll moves the task one level down
(P2 → P3), and past the last level it cancels the task.

| Gesture | Behavior |
| --- | --- |
| `Ctrl+Enter` on the Task Card or in the Schedule stage | Takes the recommended roll and closes the picker |
| `↵` on the Schedule row | Opens the date stage as before; never decays or cancels |
| `↵` on the pinned `🎲 P2 roll` row | Explicit same-level roll; counts toward the streak |
| `Ctrl+Enter` with no recommendation | On the card, a notice and no write; in the Schedule stage, behaves exactly like `↵` |
| `Ctrl+R` on the Task Card | Re-rolls the recommendation's date when it has one; no write |

A recommendation exists only for an open task whose priority value is one of
the configured levels. There is none for implicit P0 (no priority field), an
unconfigured value, plain bullets, or closed tasks.

The ladder, with `level` the current level, `streak` its roll streak, and
`limit` its roll limit (`levels[].rolls`, else `decay.rolls`, else `1`):

| Condition | Recommendation | Writes |
| --- | --- | --- |
| decay disabled (`decay: false`) | roll at `level` | date in `level`'s window, `🎲 P2 roll …` |
| `streak < limit` | roll at `level` (step `streak + 1` of `limit`) | date in `level`'s window, `🎲 P2 roll …` |
| `streak >= limit` and a next level exists | decay to the next level | priority field → next level's value, date in the next window, `🎲 P2 → P3 decay …` |
| `streak >= limit` at the last level | cancel | `[-]`, `[cancelled:: today]`, Cancel Log `🍂 decayed past P4 after 1 roll` |
| cancel recommended on a recurring task | unavailable | nothing; the picker shows a notice and writes nothing |

A decay does not count as the first roll at the new level: with the default
`rolls: 1`, every level grants two windows — entering it, then one roll.
Lifecycle with the default config (P1–P4 with windows 2–7, 8–30, 31–90,
91–365):

| # | Gesture | Entry written | Next `Ctrl+Enter` recommends |
| --- | --- | --- | --- |
| 0 | Task Card `2` (P2) | `🎲 P0 → P2 · in **12** (8–30) days` | P2 roll (1/1) |
| 1 | `Ctrl+Enter` | `🎲 P2 roll · in **20** (8–30) days` | P2 → P3 |
| 2 | `Ctrl+Enter` | `🎲 P2 → P3 decay · in **45** (31–90) days` | P3 roll (1/1) |
| 3 | `Ctrl+Enter` | `🎲 P3 roll · in **60** (31–90) days` | P3 → P4 |
| 4 | `Ctrl+Enter` | `🎲 P3 → P4 decay · in **120** (91–365) days` | P4 roll (1/1) |
| 5 | `Ctrl+Enter` | `🎲 P4 roll · in **200** (91–365) days` | Cancel task |
| 6 | `Ctrl+Enter` | Cancel Log `*<today>* — 🍂 decayed past P4 after 1 roll` | — |

The streak is derived from the Schedule Log, never stored. Only the marker's
direct child bullets are read, newest first, and each entry's reason is
classified:

| Entry (head or reason) | Class | Effect on the streak |
| --- | --- | --- |
| `🎲 <L> roll` where `<L>` is the current level, with no `→` | roll | counts; keep walking |
| `🎲 <anything> randomize` (from `bob task reroll`) | randomize | transparent: skip it and keep walking |
| `🎲 <from> → <to> decay` | decay | stops |
| `🎲 <from> → <to>` or `🎲 <L>` (a priority-level pick) | other | stops |
| `🎲 <other label> roll` (the priority was hand-edited since) | other | stops |
| a typed reason, `🤷 no reason given`, or any unparseable bullet | other | stops |

In one sentence: only reasonless, same-level recommended rolls build a
streak; any deliberate scheduling decision resets it. Conformance vectors
(current level P2, limit 1 unless noted; entries newest first):

| # | Schedule Log entries (reason only) | Streak | Recommendation |
| --- | --- | --- | --- |
| 1 | (no log) | 0 | roll P2, step 1/1 |
| 2 | `🎲 P1 → P2 · in **9** (8–30) days` | 0 | roll P2 |
| 3 | `🎲 P2 roll · in **17** (8–30) days`, `🎲 P1 → P2 · …` | 1 | decay P2 → P3 |
| 4 | `🎲 P2 randomize · in **9** (8–30) days`, `🎲 P2 roll · …` | 1 | decay P2 → P3 |
| 5 | `waiting on the API review`, `🎲 P2 roll · …` | 0 | roll P2 |
| 6 | `🤷 no reason given`, `🎲 P2 roll · …` | 0 | roll P2 |
| 7 | `🎲 P2 · in **12** (8–30) days`, `🎲 P2 roll · …` | 0 | roll P2 |
| 8 | `🎲 P1 roll · …` | 0 | roll P2 |
| 9 | `🎲 P2 roll · random in 8–30 days` | 1 | decay P2 → P3 |
| 10 | current P4: `🎲 P4 roll · …` | 1 | cancel; a recurring task is unavailable instead |
| 11 | `decay.rolls: 3`: two `🎲 P2 roll` entries, then three | 2, then 3 | roll P2 step 3/3, then decay |
| 12 | `decay: false`: five `🎲 P2 roll` entries | 5 | roll P2, no step |
| 13 | P2 has `rolls: 0`, no log | 0 | decay P2 → P3 |
| 14 | current P3: `🎲 P2 → P3 decay · …` | 0 | roll P3 |
| 15 | P4 has `rolls: 0`, current P4, no log | 0 | cancel |

The `decay` block lives on the priority property in
`~/.config/bob/config.yml`:

```yaml
- name: priority
  values: priority
  schedules: scheduled
  # Ctrl+Enter on `scheduled` takes the recommended roll. Each level allows `rolls`
  # same-level rolls (read from the task's Schedule Log); the next recommended roll
  # decays the task one level (P2 → P3), and past the last level it cancels it.
  # `decay: false` makes Ctrl+Enter a plain same-level roll that never decays.
  decay:
    rolls: 1
  levels:
    - label: P1
      value: high
      min_days: 2
      max_days: 7
      # rolls: 3   # optional: this level's own roll limit
```

`decay` absent, `true`, or `{}` means enabled with one roll per level.
`decay: false` makes `Ctrl+Enter` a plain same-level roll that never decays.
`decay.rolls` must be a non-negative integer (`0` means every recommended roll
decays); `levels[].rolls` overrides it for one level. `decay` and `rolls` are
rejected on non-priority properties, as `levels` is.

In batch sessions (`N<Ctrl+Shift+P>` and Task Link), every open target with a
configured priority gets its own recommendation from its own line and Schedule
Log, and the `scheduled` row previews the mix (for example
`4 tasks · 2 roll · 1 decay · 1 cancel`). Targets with no recommendation are
skipped and reported. One `Ctrl+Enter` applies the whole batch in a single
guarded write: one undo step in counted sessions, all-or-nothing preimages in
link sessions. If any cancel target is recurring, the whole batch is refused
and nothing is written.

To keep a task at its level, re-pick its priority level or reschedule it with
a typed reason — both reset the streak. `bob task reroll` entries never count
for or against it.

### Approved-decay decision planner

The shared approved-decay action planner (`planFreshnessDecayCard` in
`bob-navigation-hotkeys`, covered by
`scripts/test-navigation-decay-planner.cjs`) composes the recommendation,
refresh, and log-insertion planners above into one stable, previewed card
model for keep-streak decisions. It is pure: every displayed date is rolled
exactly once from an injected random source, the model is frozen, and approval
persists the preview without re-rolling. The planner never writes; the card
interaction and guarded commit landed with the decision-card phase
(`FreshnessDecayCardModal` in bob-navigation-hotkeys 1.69.0, covered by
`scripts/test-navigation-decision-card.cjs` plus the real-handler suite
`scripts/test-navigation-decision-card-handlers.cjs` in 1.70.0). The press
opens the card and writes nothing; every approval revalidates the task line,
local day, decay config, trigger eligibility, the child Schedule Log, and the
priority ladder config, then reuses the previewed plan/date through
the existing transactional writers (one undo step). Esc writes nothing and
retains the anchor; stale inputs rebuild for a fresh choice.

- **Not now (Enter).** P0 tasks enter at the first configured level in ladder
  order whose `min_days` exceeds the effective refresh interval (7 → P2,
  30 → P3 with the default ladder), unless `freshness.decay.enter` names a
  valid fixed level. Prioritized tasks reuse the recommended roll or decay
  unchanged. A terminal cancel is never offered: it substitutes a truthful
  same-level roll for this card only. Unknown priorities, unknown `enter`
  labels, invalid windows, and dates that would not defer into the future
  make Not now unavailable with an explanation instead.
- **Less often (L).** The next refresh preset strictly above the current
  interval (7 → 14 → 30 → 90); at 90+ the existing custom refresh picker
  takes over constrained to a longer value ≤ 365, and at 365 the row is
  unavailable.
- **Keep (Alt+F)** counts once with saturation at 999 and never resets;
  **Reword (E)** and **Less often** stamp and clear; **Drop (D)** cancels
  through the existing guarded cancel writer.
- **Kept-count tails.** Schedule-changing decisions append `· kept N×` after
  the existing reason head (`🎲 P0 → P2 decay · in **17** (8–30) days ·
  kept 3×`), so the classification table above keeps its meaning.
- **Review-decision entries.** Less often and Reword write dated Schedule Log
  entries (`🎲 less often · every 7 → 14 days · kept 3×`,
  `🎲 reword · kept 3×`) with no fabricated scheduled date change; both
  classify as `other` and deliberately reset the roll streak. Drop writes a
  dated Cancel Log entry (`🍂 dropped after 3 keeps`) and preserves keeps
  history on the closed line. Keep and dismissals write no log.
- **Explicit levels (1–4)** preview one roll per configured level in ladder
  order, each with reset; absent levels are never invented. An invalid
  priority config disables every priority-changing action while keeping
  Keep, Reword, Less often, and Drop.

### Schedule-log reason prompt

The log records every scheduled change the `Ctrl+Shift+P` Task Card makes; the
prompt appears only when the reason is not already known. After choosing a
`scheduled` date from a typed date or a preset in the Schedule stage, one
review opens before anything is written: Reason focused, plus optional Work
summary when any explicitly targeted Next/Pending task qualifies. Pressing `↵`
with text in Reason logs it as a dated entry under a managed
`🗓️ **SCHEDULE LOG**` child bullet on the task. Pressing `↵` on an empty
Reason depends on whether the task already has that marker: on a task with no
log yet it still writes the date only, with no entry and no marker created; on
a task that already has a `🗓️ **SCHEDULE LOG**` it records
`🤷 no reason given` as a dated entry, so the history the task is already
keeping has no gaps. The marker is the opt-in — once a task has one, its log is
complete, and a task without one is never given one by a skipped review.
Pressing `Esc` or `Ctrl+[` in the review cancels the whole modal, including the
date itself, so nothing is written.

An inline reason (`3 waiting on API`) skips the Reason field; Shift+Enter skips
the reason by the blank-reason rule without skipping an applicable Work Log.
Empty fields plus Enter skip both optional logs, and a blank Work summary still
writes no Work Log.

```markdown
- [?] #task Ship the thing [priority:: medium] [scheduled:: 2026-08-20] ^ship
  - ⛓️ **DEPENDS ON:** [[#^blocked-by-this]]
  - Some freeform note I wrote by hand
  - 🗓️ **SCHEDULE LOG**
    - _2026-08-13 → 2026-08-20_ — waiting on the API review to land
    - _2026-08-06 → 2026-08-13_ — was out sick
```

Entries are nested one level under the marker bullet and read newest first: the
top entry always answers why the task is scheduled where it is now. The
italicized date span shows the previous value on the left and the date just
chosen on the right; a task's first entry has no previous value and reads
`*<date>* — <reason>`. The marker itself is appended as the last direct child
of the task the first time a reason is logged, after any hand-written notes or
dependency links; once it exists, it is reused in place and never moved or
duplicated.

Choosing a priority level, or the pinned priority-roll suggestion in the
`scheduled` stage, never prompts: the software chose the date, so it writes its
own deterministic reason immediately. These entries, and a skipped reason
prompt on a task that already keeps a log, are marked with a leading emoji so
they read as machine-written months later — 🎲 for a date the software rolled,
🤷 for a date the user chose but declined to explain:

| Gesture                                                       | Reason text                                                             |
| ------------------------------------------------------------- | ----------------------------------------------------------------------- |
| Priority level picked, previous level differs                 | `🎲 <from> → <to> · in **<chosen>** (<min>–<max>) days` |
| Priority level picked, task had no priority field             | `🎲 P0 → <to> · in **<chosen>** (<min>–<max>) days` |
| Priority level re-picked unchanged                            | `🎲 <level> · in **<chosen>** (<min>–<max>) days` |
| Pinned roll suggestion chosen in the `scheduled` stage         | `🎲 <level> roll · in **<chosen>** (<min>–<max>) days` |
| `Ctrl+Enter` recommended decay on the `scheduled` row          | `🎲 <from> → <to> decay · in **<chosen>** (<min>–<max>) days` |
| Reason prompt skipped on a task that already has a log         | `🤷 no reason given` |
| `bob capture <text> p:<N>` rolls the scheduled date            | `🎲 P0 → <to> · in **<chosen>** (<min>–<max>) days` |
| `bob task reroll` re-rolls a due task                             | `🎲 <level> randomize · in **<chosen>** (<min>–<max>) days` |

`bob capture` has no interactive stage, so it never prompts for a reason and
never writes the `🤷 no reason given` fallback: a captured task is always a
brand-new line, so it can never already keep a log for that fallback to
append to.

`bob task reroll` rolls the same configured windows vault-wide for every due
prioritized task and records the `🎲 <level> randomize` reason above; see
[randomize.md](randomize.md).

The recommended same-level roll (`Ctrl+Enter` when the streak is under the
limit) writes the existing `🎲 <level> roll` reason, not a new one.

An automatic entry — a roll, or a skipped prompt on a task with a log — is
skipped when the resulting date equals the date the task already has, because
writing a change that did not happen would be noise. A typed reason is a
human decision and is written even when the chosen date equals the current
one. Pressing `Ctrl+D` to remove `scheduled` still writes nothing; removal is
not a reschedule. In a counted priority session (`N<Ctrl+Shift+P>` →
`priority`), each task's entry uses its own previous priority level, since
counted tasks can start from different levels. The counted `scheduled` session
(pinned roll suggestion) still applies one shared reason, because the
suggestion is only pinned when every counted task shares one configured
priority. A counted session's skipped-prompt fallback is likewise applied per
task: only the counted tasks that already keep a log get an unexplained entry,
and a task without one is left untouched exactly as it would be outside a
counted session.

### Scheduling Work Log prompt

Scheduling a Pending (`[/]`) or Next (`[*]`) task offers one optional Work
Log summary before anything is written. The prompt appears after the
Schedule Log reason stage for an explicit `scheduled` date, and directly for
a priority pick, a pinned roll, or a recommended roll/decay. It applies to
every scheduling gesture — typed dates, presets, priority rolls, pinned
rolls, and recommended rolls/decays — whether the resulting date is future,
today, past, or unchanged. Ready, Blocked, closed, and non-task bullets keep
the existing flow with no prompt, as do cancel, lane, picker refresh,
`dependsOn`, and property-deletion rows. The Pending Alt+F / Ctrl+Alt+F
refresh prompt is separate from this scheduling Work Log prompt and is
documented in `docs/freshness.md`.

The stage is titled `Schedule task` (`Schedule N tasks` for a batch). It
shows a `Work summary` input with the placeholder
`What did you get done? (optional · ↵ to skip)`, a preview of the dated
entry, where it will be saved, the frozen scheduling result, and a
`nothing written yet` note. Pressing `↵` with text commits the schedule plus
one `*YYYY-MM-DD* — <summary>` entry per qualifying task; pressing `↵` empty
commits the schedule with zero Work Log writes. A skipped prompt never
creates a marker and never writes a `🤷` fallback. Pressing `Esc` cancels the
entire gesture, including the chosen date, the Schedule Log reason, and any
Pomodoro cleanup, so nothing is written.

Only explicit targets qualify: the task under the cursor, the counted tasks,
or the linked tasks. A directly selected Pending/Next `^prj` task prompts
and logs on that lifecycle task; tasks that only receive a propagated project
schedule never get an entry. In a batch the prompt is asked once; the shared
summary goes only to the qualifying Pending/Next targets, and the preview
names the qualifying count. In a mixed recommendation batch only the eligible
roll/decay targets receive entries; cancel and skip targets never do.

```markdown
- [/] #task Ship the thing [scheduled:: 2026-10-05] ^ship
  - 🛠️ **WORK LOG**
    - *2026-10-02* — Finished the API review
```

The entry uses the same shape as lane-release Work Logs, newest first under
the task's direct `🛠️ **WORK LOG**` child. Whitespace is normalized with
Markdown preserved; `::` warns as an inline field. The notice adds a
`1 Work Log` chip when entries are written.

### Deferring a task prunes it from today's open Pomodoros

When one of these `Ctrl+Shift+P` gestures leaves a task carrying a strictly
future `scheduled` date, Bob Navigation Hotkeys also removes every live link
to that task from today's daily note that sits under an **open** top-level
Pomodoro entry:

| Gesture                                                   | Pruned targets                                                                                              |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `scheduled` → future date (typed, preset, or pinned roll) | the one task                                                                                                 |
| `priority` → P1–P4 (rolls a future date)                  | the one task                                                                                                 |
| `N<Ctrl+Shift+P>` → either of the above                   | every counted target whose own resulting date is future                                                      |
| `^prj` project `scheduled` → future date                  | every ordinary task that got a future schedule from the project, **plus the `^prj` task itself**             |

An "open" Pomodoro entry is a top-level (`- [c] ...`) checkbox line in the
daily note's `## Pomodoros` section whose status is anything other than `x`,
`X`, or `-` — the same rule
[`bob task reconcile`](task-status-hooks.md) uses to decide which entries
seed its promotion graph (see `pomodoro::open_ledger_task` in `bob-cli`).
Leaving a deferred task's link under one of these entries would otherwise
keep promoting its whole dependency chain and keep it registering as recent
activity, even though it is no longer part of today's work.

A struck link (`~~[[Tasks#^x]]~~`) is a retired record of work already done
and is never touched, and entries that are already closed or cancelled
(`[x]`, `[X]`, `[-]`) are skipped entirely — nothing under them is ever
pruned. When the link is a bullet's entire body (aside from an optional
leading `🍅 ` marker), the whole bullet — and anything nested under it — is
removed; otherwise only the matched link token is removed and the bullet's
remaining text survives. Only **today's** daily note is touched; a link in a
previous day's ledger is left alone. Pruning is one-way: pressing `Ctrl+D` to
remove a task's `scheduled` value never restores a link that a prior deferral
removed, and re-adding one is the same manual `Ctrl+Shift+O`/`^^` gesture used
to link it the first time.

```markdown
## Pomodoros

- [ ] Current (0900-0930)
  - [[Tasks#^ship]]
  - [[Tasks#^stay]]
```

Deferring `^ship` to a future date leaves only the task that is still part of
today's work:

```markdown
## Pomodoros

- [ ] Current (0900-0930)
  - [[Tasks#^stay]]
```

The picker's notice reports what happened with a `removed N Pomodoro link`
chip; if the daily note changed underneath the picker between the snapshot
and the write, the schedule itself is still kept and the notice instead shows
a `not removed` chip — the write is never retried automatically.

### Cancelling a task

The `Ctrl+Shift+P` Task Card (`bob-navigation-hotkeys:set-bullet-property`)
offers **Cancel** as `x`. It appears only when at least one target is an open
`#task` (`" "`, `*`, `/`, or `?`); it is hidden on closed tasks, plain
bullets, and anywhere the cursor is not on a task or Task Link. The key opens
a reason stage — nothing is written yet. Type an optional reason and press
`↵` to cancel; `Esc` or `Ctrl+[` at either stage writes nothing. The decay
card accepts `x` as an alias for existing `d` Drop.

The row supports the picker's three target modes: the `#task` line under the
cursor, a counted session (`N<Ctrl+Shift+P>` cancels the current task plus
the next N tasks, skipping already-closed targets), and a Task Link session
(cancels the linked open tasks in their own notes). `^prj` lifecycle tasks
are allowed; `bob projects sync` already maps `[-]` to `status: canceled`.

Each cancelled task is rewritten as `[-]` with `[cancelled:: YYYY-MM-DD]`
upserted before any trailing `^block-id` (replacing an existing `cancelled`
field), matching what Obsidian Tasks itself writes so Tasks queries, `done`
filters, and `### Done & Canceled` grouping treat it the same. Nothing else
on the line changes: `scheduled`, `dependsOn`, `id`, `priority`, `created`,
and `#hide` are untouched (a cancelled task drops out of every lane view by
itself since Cancelled counts as done).

```markdown
- [-] #task Add new `Agents` sub-tab to `Artifacts` tab! [priority:: high] [created::
  2026-08-14] [cancelled:: 2026-09-30] ^agents-tab
  - ❌ **CANCEL LOG**
    - _2026-09-30_ — Superseded by [[sase_art_links_panel#^agents-sub-tab]]
```

The reason is recorded in a managed `❌ **CANCEL LOG**` child (the `❌` is
U+274C, written without a variation selector). Its entries use the same
`*YYYY-MM-DD* — <reason>` shape as the Schedule Log, newest first. A new log
is inserted as the task's **first** direct child — the verdict reads first —
while a task that already keeps one (cancelled, reopened, cancelled again)
gets the new entry prepended under the existing marker, wherever it sits. A
marker nested under a grandchild does not count. An empty reason writes no
log, unless the task already keeps one: then it gets
`*YYYY-MM-DD* — 🤷 no reason given`, the same "a kept history has no gaps"
rule the Schedule Log uses.

Past the last decay level, `Ctrl+Enter` on the `scheduled` row cancels with
the reason `🍂 decayed past <level> after <n> roll(s)` — just
`🍂 decayed past <level>` when the streak is 0; see
[Recommended roll and priority decay](#recommended-roll-and-priority-decay).

Side effects apply immediately, for feedback. Every live link to a cancelled
task with a block ID is removed from today's open Pomodoros with the same
dedicated-bullet/token semantics as the deferral prune above (a dedicated
link bullet goes with its subtree; otherwise only the link token goes), and
[`bob task reconcile`](task-status-hooks.md) stays authoritative for the
same rule. After the writes land, the picker reuses the Task Status Cycler
`api.recoverBlockedDependents` recovery: a Blocked dependent with no
remaining open dependency and no future `scheduled` date becomes Ready. The
notice card summarizes the result with `removed N Pomodoro links`,
`unblocked N dependents`, and plan-budget chips as applicable.

Refusals write nothing: recurring tasks (a `[repeat:: …]`, `(repeat:: …)`, or
`🔁` line — cancel those with Obsidian Tasks so the next occurrence is
handled), stale preimages (the note changed under the picker), and a failed
Pomodoro prune (reported as a `Pomodoro links not removed` warning chip,
never rolled back). A single cancel is one editor undo step.

### Inbox routing

On an open inbox task, `Ctrl+Shift+P` and `Ctrl+Shift+Enter` ask where the
task goes as the last step before they write. An inbox note is `inbox.md`
itself or an area note whose frontmatter `parent` resolves to `inbox.md`
(today `mac_inbox.md` and `gkeep_inbox.md`); only direct children count, so
a project filed under an inbox is not an inbox note.

**Gesture order.** The Task Card opens exactly as today, with one extra
muted `Inbox` header chip (`Answers ask where this task goes first`) while
routing is armed. Pick an action and fill in its stages as today; at the
moment the card would write, the route picker appears instead. For
`Ctrl+Shift+Enter`, the block-ID prompt still comes first when one is
needed, then the route picker appears right before the link or unlink
write. Choosing a destination performs the write and the move. `Esc` in the
route picker returns to the exact Task Card surface it came from (card or
stage, typed input intact) with nothing written, or cancels the whole
`Ctrl+Shift+Enter` toggle with nothing written — not even a new block ID.

**Route picker.** Header icon `inbox`, title `Route out of <inbox
basename>`, subtitle `<task text> · then <action>` (`<N> tasks · then
<action>` for counted sessions), placeholder `Where does this go? Filter
areas and open projects`. Destinations are the `Ctrl+Shift+M` areas and
open projects minus every inbox note: the picker never offers another
inbox. Keys: `↑↓` select, `↵` moves and applies, `⇧↵` applies in place and
keeps the task in the inbox (today's behavior, for when the right home
does not exist yet), `Esc` / `Ctrl+[` goes back (Task Card) or cancels
(`Ctrl+Shift+Enter`). On `↵` the picker preflights the move against the
live source and destination (destination still an area or open project, a
`## Tasks` section where a project needs one, no block-ID collision); a
refusal shows a Notice and keeps the picker open, with nothing written.

**Act then move.** The action's existing writer runs unchanged, in place, in
the inbox note; then nav re-discovers the routed tasks from the same start
line and moves them with the existing move engine (children carried, block
links rewritten vault-wide, freshness stamped like `Ctrl+Shift+M`). If the
action is refused, fails, or writes nothing, nothing moves. If the action
committed but the move fails, the action stays, the task stays in the
inbox, and the notice ends with `· still in <inbox>` (recoverable with
`Ctrl+Shift+M`). Unlike `Ctrl+Shift+M`, a routed move never focuses the
destination or parks the walk: the cursor stays in the inbox note on the
next inbox line.

**Notices.** Task Card off the walk: the action's rich notice card, then
`Moved to <dest>` (`Moved <N> tasks to <dest>` counted). On a landing: the
rich card, then the walk toast with `Moved to <dest>` first. `Ctrl+Shift+Enter`:
one toast, the existing link/unlink text with `· moved to <dest>` appended
(preamble of the walk toast on a landing).

**What does not route.** Closing gestures (the Task Card Cancel row `x`,
a cancelling `Ctrl+Enter` recommendation, decision-card Drop — a closed
task leaves the inbox through `bob task archive`); closed tasks, non-task
bullets, non-inbox notes; Task Link sessions; the Alt+F decision card and
its Less often stage; `Ctrl+Shift+M`, `^^` linking, `bob capture`, and the
Mac Capture task-toggle mirror. A routed answer on a landing advances the
walk exactly like any other answer (`route` outcome); if the move did not
commit, the walk settles exactly as today for that action.

## Warnings

Warnings do not make the command fail and are not auto-fixed:

- An active project has no `^prj` task.
- The `^prj` description is still
  `<short_project_completion_criteria_goes_here>`.
- An ordinary task has multiple inline `scheduled` fields. That line is left
  unchanged until it contains exactly one.

Terminal projects are allowed to be missing `^prj`; `bob task archive` may
archive the checked or canceled task later.

## Examples

Preview changes:

```bash
bob projects sync --dry-run
```

Use a temporary vault fixture:

```bash
bob projects list --bob-dir /tmp/bob-vault
bob projects sync --dry-run --bob-dir /tmp/bob-vault
```

Typical action output:

```text
  ok sase_blog  status: wip -> done  ^prj task checked
  ok bob        removed #hide from ^prj  no non-hidden open tasks or open sub-projects
  ok athena     added #hide to ^prj  project has open sub-projects
  ok athena     added [[sase_blog]] to ^prj  open sub-project
  ok athena     updated [[sase_blog]] on ^prj  sub-project completed
  ok athena     updated [[old_plan]] on ^prj  sub-project canceled
  ok athena     removed [[old_child]] from ^prj  no longer a sub-project
  ok athena     added 🗓️ [[future_child]] to ^prj  sub-project scheduled in future
  ok athena     removed 🗓️ [[due_child]] from ^prj  sub-project no longer scheduled in future
  ok athena     updated sub-projects on ^prj  canonical format
  ok roadmap    scheduled 4 tasks 2026-07-16  frontmatter scheduled is future
  ok roadmap    removed #hide from 4 tasks  task schedules replace #hide
  hint: run `bob task reconcile` to reconcile derived [?] Blocked markers
  warning outlive  active project has no ^prj task  add `- [ ] #task #prj <completion criteria> #hide ^prj`

11 projects - 1 status updated - 9 ^prj edited - 4 task schedules updated - 1 warnings
```

Schedule propagation is reported once per project rather than once per task.
The summary totals task lines whose inline schedule was added or updated;
legacy `#hide` removals are reported separately.
