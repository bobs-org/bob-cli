# Task requirements

A task can carry **requirements** ("reqs" in compact UI): checkbox children
without `#task`, nested as deep as the work needs, drawn as a quiet `⌗⌗` run
in the task's ink. They are met when the task closes and can never be ticked
early. They can be dropped with a reason or carried into their own task, and
the close card audits them once.

This file is the contract every implementation cites. The Rust side is
`src/native/task_reqs.rs` (pure recognizer), surfaced through the task block
layout, capture grammar, carry, and the successor card in later phases. The
JavaScript twins port the recognizer, layout, and writers against the copied
vectors. If an implementation disagrees with a vector, fix this doc first in
the same change.

> Part of this task's scope. Met when the task closes. Drop it or make it a task.

## 1. What a requirement is

A **requirement** ("req") is a Markdown list item that meets all of these:

1. Its body starts with a checkbox (`[c] `).
2. It does not carry the global filter token (`#task`), matched as a whole
   token the way `note_tasks` matches it (`#tasks` and `#taskbar` are not
   the filter).
3. Its **structural parent**, the nearest shallower list item, is a real
   `#task` line or another req.
4. It is outside code fences (including fences nested in list items) and
   outside quote lines.

Following structural parents up the chain reaches exactly one real task, the
req's **owner**. Depth is 1 for a direct child of the owner, and each req
ancestor adds 1. Depth is computed, never stored, so re-indenting a branch
keeps it right.

```text
- [ ] #task Ship the exporter ^export          ← owner
	- ⛓️ **DEPENDS ON:** [[#^parser]]          ← links
	- [ ] Handles supported inputs              ← req, depth 1
		- [ ] Handles Windows files             ← req, depth 2
			- [ ] Preserves CRLF                ← req, depth 3
			- Fixture: windows.csv              ← note of "Handles Windows files"
		- [ ] Handles empty input               ← req, depth 2
	- [-] Benchmarks within 5% of v1            ← dropped req
		- ❌ **CANCEL LOG**
			- *2026-11-21* — v1 is gone
	- [>] [[#^export-docs|Docs cover every flag]]  ← carried req
	- NOTES                                     ← drawer (NOTES first)
		- Morgan reported the original issue.
	- 🛠️ **WORK LOG**                           ← logs
```

These are **never** reqs:

- a checkbox under a note, a drawer, a managed link line, or a log;
- a checkbox in a notebook, a Pomodoro row, a bank row, a fence, or a quote;
- any line that carries `#task`. A nested `#task` child is a **subtask**,
  which is the existing contract term (`subtasks`, `open_subtask`). It starts
  its own ownership boundary and owns its own reqs.

A plain note, drawer, or log between a checkbox and the task breaks the
chain. A pasted checklist under `NOTES`, or under a req's note, therefore
stays an ordinary checklist. "Any checkbox somewhere below a task" is
explicitly **not** the rule.

A req **never has**:

- `#task`, `[created::]`, a lane, freshness, priority, a schedule, or
  dependencies;
- Task Links, walk rows, Ready-cap or Today membership, or stats.

A req **may have**:

- child notes, child reqs, a trailing `^anchor`, and a ❌ CANCEL LOG child
  when dropped.

The moment a req needs any of the "never has" items, it is carried into a
real task. A `^prj` task may own reqs; they are the project's completion
criteria.

**Safety bound.** Depth above `MAX_REQ_DEPTH` (16) is reported as `req_depth`,
and writers refuse it. Nothing is flattened or dropped silently.

## 2. State model: met is derived, never written

| Bytes on the req                       | Owner open                        | Owner done (`[x]`) | Owner cancelled (`[-]`) |
| -------------------------------------- | --------------------------------- | ------------------ | ----------------------- |
| `[ ]`                                  | **Pending**                       | **Met** (derived)  | **Moot** (derived)      |
| `[-]`, plus an optional ❌ CANCEL LOG  | **Dropped**                       | Dropped            | Dropped                 |
| `[>] [[#^id\|Alias]]`                  | **Carried**                       | Carried            | Carried                 |
| `[x]` / `[X]` (hand-written or legacy) | Met                               | Met                | Met                     |
| any other mark                         | Pending, flagged, never rewritten | Met                | Moot                    |

**Ancestor rule.** A req whose ancestor req is Dropped or Carried is
**Excluded**, and the effective state records which ancestor excluded it.
Its bytes stay unchanged. Excluded reqs are never called met. They return to
Pending when that ancestor is restored. A descendant that was dropped or
carried on its own keeps that state.

A hand-ticked `[x]` on an intermediate req is Met for itself only. Its
pending descendants stay Pending while the owner is open.

**What follows from this:**

- **No Bob writer ever writes req completion.** The only req writes are
  drop (`[ ]` → `[-]` plus a Cancel Log), restore (`[-]` → `[ ]`), and
  carry (the line becomes a `[>]` stub).
- **Reopening is free.** Reopening the owner makes its `[ ]` reqs pending
  again without any write.
- **Every close path is correct by construction**: capture, Ctrl+Enter,
  Alt+], gcal past-day sync, collect-done, mobile, raw edits.
- **The accepted cost.** Raw or GitHub views of `done/` show `[ ]` boxes
  under done tasks.

## 3. Task block layout and the NOTES drawer

One shared module, `task_block_layout`, answers one question: "where does a
new child of kind K go?". It has a JS twin backed by RL vectors. (Shipped in
the `block-layout` phase; the zones below are normative now.)

The zones, in order:

1. **Cancel Log.** It stays first wherever a Cancel Log exists. This is the
   existing contract.
2. **Managed links.** DEPENDS ON, BLOCKS, SUB-PROJECTS, LINKS, LINKED FROM.
3. **Reqs.** Each req moves with its whole subtree.
4. **Authored.** Loose notes and nested `#task` subtasks, in authored order.
5. **Drawers.** `NOTES` comes first, then the other drawers in authored
   order.
6. **Logs.** SCHEDULE, WORK, ROTTEN, in their existing relative order.

**Zone rules:**

- Writers insert into the right zone.
- Order _within_ a zone is authored order and is never re-sorted.
- Under a req, children keep authored order. Reqs and notes interleave
  freely there, and a req never gets its own `NOTES` drawer.
- Order is a writer rule plus an explicit `bob task tidy`. It is never a
  validity rule and never a silent reorder.

**The NOTES rule.** A task that owns at least one req (any state, any depth)
keeps its notes in a `NOTES` drawer:

- The gesture that gives a task its **first** req files that task's loose
  direct notes into `NOTES` in the same write. Only the task's own notes
  move, never notes attached to reqs. The write is previewed in capture and
  is one undo step in Obsidian. This is the "first-req scoop".
- Later Bob-written notes to a req-owning task file into `NOTES`.
- `@route+id#drawer` and explicit drawer bullets still win.
- Zero-req tasks keep loose notes exactly as today.
- Hand edits in Obsidian are never reshuffled. `bob task tidy` fixes drift
  on request.

`NOTES` is created only when a note lands in it: the scoop moved at least
one note, or a note is being written. A req-owning task with no notes has no
`NOTES` header. This keeps the Task Drawer contract: a drawer is nonempty by
construction.

## 4. Rendering

**Glyph.** The run reuses the tag-mark hash mask, tightly kerned: depth 1
draws `⌗⌗`, depth 2 `⌗⌗⌗`, and depth 3 or more `⌗⌗⌗⌗`. A `#task` draws `⌗`,
so a req is the same family one step down.

**Ink.** The run uses the owner's hash ink: the owner's type hue, or the
default teal. Ink never encodes state.

**State styling.** All of these rules are scoped to `[data-bob-req]`:

| State      | Look                                                                                                                                                                            |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pending    | Full ink.                                                                                                                                                                       |
| Met / Moot | The owner's done or cancelled line treatment, with the run faded.                                                                                                               |
| Dropped    | Muted, struck text. Its Cancel Log shows beneath. An explicit override stops the vault's `task-statuses.css` from painting its full-width grey `data-task="-"` bar on req rows. |
| Carried    | The run plus a small `→`. The stub's wikilink keeps its live Task-Link status mark, so the carried task's lane shows in place.                                                  |
| Excluded   | Muted and not struck. The tooltip says `goes with “{ancestor}”`.                                                                                                                |

**Mechanics (Obsidian).** Live Preview uses `Decoration.line` attributes,
following the Blocked-Marks pattern. Reading view uses an attribute-only
post-processor. Neither creates DOM nodes. The checkbox `input` gets
`appearance: none`, `pointer-events: none`, and the run as its mask, so it
cannot be clicked. Moving the cursor onto the marker reveals the raw
`- [ ] `. The tag-marks session toggle restores native checkboxes. The
tooltip and accessible label read
`Requirement · level {d} of “{Owner}” · {state}`, followed by the teaching
line. (Shipped in the `ledger-marks` phase.)

**Mac.** `capture-parse` emits a `req_marker` span over the `[ ] ` bytes,
carrying a `depth`. The Mac hides those bytes and draws the run in their
four cells, using the record-route overlay pattern. Caret adjacency reveals
the raw bytes. (Shipped in the `mac-authoring` phase.)

**Cards.** Successor cards draw the same run before each req row's text.

## 5. Authoring

A `- [ ] text` bullet is a req wherever its structural parent is a task or
a req: under a new task item, under a note-less `@route+id`, nested under a
req bullet at any depth, nested under a project-note capture's task bullet,
and in the same-line `[ ]` form. A plain bullet under a req bullet is a note
of that req. Capture's authored depth is numeric; levels past 2 are admitted
only beneath a req chain. The `..` alias and Ctrl+G T from the canceled
epic are **not** built.

Each refusal is reported on the bullet's byte range in `capture-parse` and
as a `code` in `bob capture` errors:

| Code         | Situation                                        | Copy                                                              |
| ------------ | ------------------------------------------------ | ----------------------------------------------------------------- |
| `req_in_drawer` | inside a drawer bullet or on a `#drawer` route | `Drawers hold notes — put the requirement directly under the task` |
| `req_parent` | under a note bullet                              | `A requirement needs a task or requirement directly above it`      |
| `req_state`  | pre-checked (`[x]`, `[-]`, or another mark)      | `Requirements start open — they're met when the task closes`      |
| `req_link`   | a Task Link whose anchor resolves only to a req line | `Requirements aren't planned — link “{Owner}”, or Ctrl+Shift+T makes it a task` |
| `req_depth`  | beyond `MAX_REQ_DEPTH`                           | `Requirements nest at most 16 deep — carry this branch into its own task` |

Notebook routes still refuse checkboxes, and `@route` task lists still strip
`[ ]`. Text copied from outside writers (gcal, gkeep, gmail, highlights,
refs) escapes a bullet-start checkbox as `\[`, so pasted text never becomes
a requirement. (Shipped in the `capture-reqs` and `reader-hardening`
phases.)

The typing rewrite `expand_req_marker` works on `--cursor` typing at an
eligible bullet start: typing the space in `- [` rewrites the marker to
`- [ ] `, with the caret after it. It is one native, undoable edit. A `]`
typed immediately after an expansion is absorbed, and so is a space typed
immediately after that `]`; absorption stops at the first other keystroke
or caret move. Backspace with the caret right after a bullet-start `[ ] `
removes the whole marker and leaves `- `. Paste, templates, and
`bob capture` never rewrite. (Shipped in the `capture-reqs` phase; Obsidian
and Mac implement the same described behavior.)

## 6. Carry: a req becomes its own task

Carry reuses Task Split. Any Pending req at any depth can be carried. The
new task is `- [ ] #task <text> [created:: today] ^<id>` placed after the
owner's block; a trailing `^anchor` is reused as the ID. The branch travels
whole: descendant reqs become the new task's reqs with rebased depth, and
the req's direct notes become the new task's notes under the NOTES rule. The
req line becomes the stub `- [>] [[#^id|Alias]]` at the same position, and
the new task gets a `🪢 LINKED FROM: [[Owner]]` back-link. Nothing is
inherited and no dependency is added. Selecting an ancestor and one of its
descendants together is refused (`split_overlap`). (Shipped in the
`req-split` phase; see [Split bullets out into tasks](task-split.md).)

## 7. The close card

A managed Done gesture opens the card when its closing set includes a
non-recurring task with at least one Pending req. One `requirements` group
per owner, placed first, lists every Pending req in document order,
indented by depth, with the run, text, and consequence strip. A branch folds
only when its root gets drop or carry. Keys: Ctrl+X met, Ctrl+Shift+X drop
with an inline reason, Ctrl+Shift+T carry into its own task, Space/Tab carry
plus pull-in, Ctrl+J jump pin. Overlapping picks are refused on the row, and
stale rows (reordered, re-indented, or edited duplicates while the card is
open) refuse as `req_close_stale`. With no flags, Pending reqs are Met with
no write. (Shipped in the `close-engine` phase; wire details live in
[Task dependency links](task-dependencies.md) §12.)

## 8. Promotion moves the boundary one level

Ctrl+Shift+Alt+N seeds the new project note from the task's direct
children: outer Pending reqs become `## Tasks` rows (their descendant reqs
stay reqs), dropped roots become `[-]` rows, legacy `[x]` rows keep their
mark, `[>]` stubs become `## Notes` lines verbatim, and loose notes plus
`NOTES` merge into one `## Notes` section — even on tasks with no reqs.
(Shipped in the `promotion` phase.)

## 9. Vault migration

`bob task rename-drawer REQUIREMENTS NOTES` retires the old name everywhere
(open, closed, and archived tasks, plus `^prj` `## Requirements` sections,
which become `## Notes`), preserving content byte for byte; drawer bullets
never become reqs. Where the owner already has `NOTES`, the two merge, and
heading links are repaired. One `bob task tidy` pass then repairs open task
blocks into canonical order. Both run as gated, previewed, one-commit
writes. (Shipped in the `drawer-rename`, `task-tidy`, and
`vault-migration` phases.)

## 10. Vectors

- **RQ** (`tests/fixtures/task_reqs/vectors.json`): the recognizer — owner
  and depth, every mark under open/done/cancelled owners, excluded
  descendants, the depth bound, anchors, whole-token `#task` matching, row
  keys, subtree ends, and note ranges.
- **RL** (`tests/fixtures/task_reqs/layout.json`): block-layout slots and
  tidy before/after. (Added in `block-layout`.)
- **RW** (`tests/fixtures/task_reqs/writes.json`): drop, restore, toggle,
  scoop, and both `notes_drawer_birth` branches. (Added in `block-layout`.)
- **RV** (`tests/fixtures/req_close/vectors.json`): close-card row building,
  verdict bytes, overlaps, staleness, jump landings, and copy strings.
  (Added in `close-engine`.)
