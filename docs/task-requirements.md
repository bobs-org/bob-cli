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

One shared module, `task_block_layout` (`src/native/task_block_layout.rs`),
answers one question: "where does a new child of kind K go?". It has a
JavaScript twin backed by the RL vectors, and its requirement writers (below)
are backed by the RW vectors.

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
  move, never notes attached to reqs, and nested `#task` subtasks stay where
  they are. The write is previewed in capture and is one undo step in
  Obsidian. This is the "first-req scoop".
- Later Bob-written notes to a req-owning task file into `NOTES`. A capture
  note, a carry, and a promotion row all go through `note_home`.
- `@route+id#drawer` and explicit drawer bullets still win.
- Zero-req tasks keep loose notes exactly as today (see below).
- Hand edits in Obsidian are never reshuffled. `bob task tidy` fixes drift
  on request.

`NOTES` is created only when a note lands in it: the scoop moved at least
one note, or a note is being written. A req-owning task with no notes has no
`NOTES` header. This keeps the Task Drawer contract: a drawer is nonempty by
construction. Reviewer decision `notes_drawer_birth = with_first_note`.

**Zero-req tasks keep today's placement.** A task with no reqs files a loose
note at the tail, before its first Schedule, Work, or Rotten Log (else at the
end of the block), and its authored notes and drawers may interleave: capture
has always written notes after the drawers, and tidy never reshuffles that
history. For such a task only the Cancel Log, managed-link, and log zones are
enforced. The NOTES-first drawer rule applies to a task that owns a req.

### 3.1 Insertion slots

`insertion_offset(contents, task, kind)` returns a slot: the 0-based line to
insert before (the line count appends), the byte offset of that line, and the
indentation. The indentation is the first existing child's, else the note's
dominant indent unit under the task, else a tab. Blank lines inside a child's
extent travel with that child, and a slot never lands after a child's
trailing blank lines.

| Kind           | Slot                                                                                                                               |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `cancel-log`   | The task's first child, directly after the task line.                                                                              |
| `managed-link` | After the last Cancel Log or managed-link subtree; else before the first child. Finer ordering inside the zone is the link writers'. |
| `req`          | After the last req's subtree; else the managed-link slot.                                                                          |
| `authored`     | A task that owns reqs: after the last child up to the authored zone. A task with no reqs: the tail slot.                           |
| `drawer`       | Before the first Schedule, Work, or Rotten Log; else the end of the block.                                                         |
| `notes-drawer` | Before the first drawer; else the `drawer` slot.                                                                                   |
| `log`          | The end of the block.                                                                                                              |

`note_home(contents, task)` answers where a Bob-written loose note files:
`loose` (a task with no reqs: the `authored` tail slot), `notes` (the task's
existing `NOTES` drawer), or `new-notes` (create `NOTES`, first among the
drawers, then file in it). `ensure_drawer` routes through the same slots, so a
created `NOTES` lands first among the drawers and every other created drawer
keeps its place before the logs.

The link and log writers already agree with the zones, and a test pins that:
`blocks_insert_slot` and `relation_insert_slot` place BLOCKS, LINKS, and
LINKED FROM inside the managed zone, the Depends-On line is the first child
except after a Cancel Log, and a new managed log appends as the last child.

### 3.2 Violations and tidy

`layout_violations` reports each child that sits after a child of a later
zone (`out-of-zone`), a `NOTES` drawer that is not first among the drawers
(`notes-not-first`), and, on a req-owning task, a loose note outside `NOTES`
(`note-outside-notes`).

`plan_tidy(contents, task, policy)` is a pure plan for one **open** task
block; a done or cancelled task is never tidied, because history is never
rewritten. It:

1. files a req-owning task's loose direct notes into `NOTES` (creating it
   only when a note moves), rebasing their indentation under the drawer;
2. moves whole child subtrees into their zones, with a stable sort so authored
   order survives inside each zone and `NOTES` leads the drawers.

It returns `None` when the block is already canonical, so a second run
reports nothing. The plan carries `moves` (the fewest subtrees that changed
place), `notes_filed`, and whether `NOTES` was created. A repair shifts the
lines below it, so a caller applies one task at a time and rescans.

### 3.3 Requirement line writers

Each writer is a pure function over the whole note. The caller names the line
and the exact text it expects there (the preimage); a moved or edited line
refuses as `req_stale` rather than writing somewhere else. None of them ever
writes req completion.

- **`drop_req`** rewrites `[ ]` (or a flagged mark) to `[-]` and writes the
  reason as a Cancel Log, the first child of the req, in the task cancel
  shape: `- ❌ **CANCEL LOG**` over `- *YYYY-MM-DD* — reason`, newest first. An
  empty reason writes no log unless the req already keeps one, which then
  gains `*YYYY-MM-DD* — 🤷 no reason given`. A req under a done owner can be
  dropped (the post-close card does so). It reports how many descendant reqs
  went with it; their bytes are unchanged.
- **`restore_req`** rewrites `[-]` back to `[ ]` and keeps the log, the way
  reopening a task keeps its log.
- **`toggle_note_req`** is Ctrl+]. A note becomes a req: at depth 1 the line
  and its subtree move to the end of the reqs zone, and the task's first req
  runs the first-req scoop in the same write; deeper, the toggle happens in
  place. A req becomes a note: a leaf converts in place at depth 2 or more,
  and at depth 1 the line and its subtree move into `NOTES`. A req with
  descendant reqs refuses.
- **`insert_reqs`** inserts depth-1 req blocks (relative indentation, each
  opening with `- [ ] text`) at the end of the reqs zone and runs the
  first-req scoop when the task had no req before.

| Code              | Refused when                                                              |
| ----------------- | ------------------------------------------------------------------------- |
| `req_stale`       | The line no longer holds the expected text.                               |
| `not_a_req`       | Drop or restore on a line that is not a requirement.                      |
| `req_dropped`     | Drop or toggle on a dropped req.                                          |
| `req_carried`     | Drop, restore, or toggle on a `[>]` stub: `Carried — open the linked task`. |
| `req_met`         | Drop or restore on a hand-ticked `[x]` req.                               |
| `req_excluded`    | Drop under a dropped or carried ancestor.                                 |
| `req_not_dropped` | Restore on a pending req: the owner's Ctrl+Enter copy.                    |
| `req_has_nested`  | Toggle to a note on a req with nested reqs.                               |
| `req_in_drawer`   | Toggle on a note inside a drawer.                                         |
| `req_parent`      | Toggle under a note, a managed line, or a checklist that is no req.       |
| `not_a_note`      | Toggle on a drawer header, a nested task, or a managed line.              |
| `req_depth`       | A write that would nest beyond `MAX_REQ_DEPTH`.                           |
| `req_state`       | Insert of a block that is not an open `- [ ]` req.                        |
| `req_empty`       | Insert with nothing to insert.                                            |
| `not_a_task`      | Insert under a line that is not a task.                                   |

Only creating writers refuse depth: drop and restore are state edits and work
at any depth.

### 3.4 Tidying task blocks

`bob task tidy [ROUTE...]` is the explicit, on-request repair of layout drift.
It runs `plan_tidy` (§3.2) over every **open** `#task` block in the vault, so
whole subtrees move into their zones and a req-owning task's loose notes are
filed into `NOTES`. Nothing else ever reorders a block: no writer, no
`bob task reconcile`, and no background pass.

- **Scope.** Every note except archive history under `done/`, plus the
  always-excluded `_generated/`, `_templates/`, `_conflicts/`, and dot
  directories. Done and cancelled tasks are history and are never rewritten;
  an open subtask under a closed task is still tidied. Each `ROUTE` narrows the
  scope to one note: a capture route such as `bob` or `@bob`, or a
  vault-relative path with or without `.md`. A route that names no note
  (`unknown_route`) or a note out of scope (`route_out_of_scope`) exits 2
  before anything is planned.
- **One pass per note.** Tasks are planned top to bottom. A repair rewrites
  only lines inside its own block, so the pass rescans after each repair and
  resumes just below that task line; a nested subtask moved by its parent is
  repaired next, in the same run.
- **Dry run by default.** A bare run takes no lock, writes nothing, runs no
  Git, and does no sync. Human output groups the repaired tasks by note, each
  with a colored before/after excerpt of its block, and ends with
  `{t} tasks in {n} notes · {m} moves · {s} notes filed under NOTES`.
  `-f json` reports the same plan (`schema_version: 1`; see
  [JSON outputs](reference/json-outputs.md)); each task's `line` is where the
  task line sits after the repair.
- **Apply.** `-a` takes the shared vault lock, pre-syncs, replans, writes every
  note guarded by its full preimage (a note edited since planning fails as
  `stale_note` and nothing is written), verifies that every rewritten note
  plans no further repair, makes one scoped commit
  `bob task tidy ({t} tasks in {n} notes)`, and post-syncs only when it
  committed.
- **Safety.** A planned note must keep every nonblank line exactly once,
  indentation aside, and may only add the `NOTES` headers it reports. A note
  that fails the check, or that a second pass would still change, is left
  unchanged with a warning.
- **Idempotent.** A second run reports nothing.

### 3.5 Renaming drawers

`bob task rename-drawer OLD NEW` renames a drawer everywhere, as one
previewed, idempotent commit. The `REQUIREMENTS` → `NOTES` migration (§9) is
one approved run of it; it is not a one-off script.

- **Names.** OLD matches by drawer key, so case and spacing do not matter.
  NEW is written in the drawer catalog's casing: ALL-CAPS for a task drawer
  (`NOTES`), Title Case for a `##` section (`Notes`). A NEW that is reserved
  for a managed line or `TASKS` (`reserved_drawer`) or is not a valid drawer
  title (`invalid_drawer_title`), an OLD that names a managed line, and an OLD
  equal to NEW (`same_drawer`) exit 2 before anything is read.
- **Scope.** Every `#task` drawer, whether its task is open, done, cancelled,
  or archived under `done/`, plus every eligible `##` section of a project
  note (the `^prj` task's drawers; structural sections never are). Ordinary
  headings in other notes, other heading levels, fenced code, quotes, and
  drawer-looking bullets that are not a task's direct child are left as
  written and listed as excluded. `_generated/` is never read, and
  `_templates/` hits are reported only, never edited.
- **Merge.** Where one owner (a task, or a project note) already holds NEW,
  or holds OLD more than once, the containers become one at the earliest
  participating position, labelled NEW. Bodies follow in original order:
  repeated content, anchors, checkbox bytes, logs, and nesting are kept, and
  an entry moved from a differently indented drawer takes the kept drawer's
  indentation. A section body that is not a list follows a blank line. No
  bullet ever becomes a requirement.
- **Links.** `[[note#Old]]`, `![[note#Old]]`, and Markdown `note.md#Old`
  links into a renamed project note are repaired, nested heading paths
  included; block links (`#^id`) and links inside code never change. A link
  whose note cannot be resolved or was refused is reported and left as
  written.
- **Refusals.** A project note whose rename would leave a heading ambiguous
  is refused whole, with the reason: another heading already named OLD or
  NEW, two merged sections bringing the same subheading together, or a
  structural NEW such as `Events`. A note whose planned rename would drop or
  invent a line is refused the same way.
- **Report.** The dry run prints each change with a colored excerpt, then
  the refused notes, excluded lookalikes, unresolved links, and template
  hits, and ends with
  `{d} drawers ({o} open · {c} closed · {a} archived) · {s} project sections · {m} merges · {l} links repaired in {n} notes`.
  `-f json` reports the same (`schema_version: 1`; see
  [JSON outputs](reference/json-outputs.md)).
- **Apply.** `-a` takes the shared vault lock, pre-syncs, replans, writes
  every note guarded by its full preimage, verifies that nothing replans,
  makes one commit `bob task rename-drawer: OLD → NEW ({n} notes)`,
  post-syncs only when it committed, and refreshes the drawer catalog. A
  second run reports nothing.

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
`[ ]` from the list bullet itself.

Positions, deeper nesting beneath req chains, the `req_marker` span with its
`depth`, and the `req_in_drawer`, `req_parent`, `req_state`, and `req_depth`
refusals shipped with capture grammar 29 (see
[Requirement bullets](capture.md#requirement-bullets)). Until the req writes
land, an admitted req is written at its authored depth in authored order.
Placement in the reqs zone with the NOTES rule, the first-req scoop, and the
`req_link` refusal land later in the `capture-reqs` phase.

**Pasted text never becomes a requirement.** Text copied from outside
writers (gcal descriptions and private notes, gkeep, gmail, highlights, ref
tasks) escapes a bullet-start checkbox as `\[` (`- \[ ] item`). It renders
the same words and is never a checkbox. (Shipped in the `reader-hardening`
phase.)

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

Carry reuses [Task Split](task-split.md) (`src/native/task_split/`); the
requirement side lives in `task_split/carry.rs`.

- **What can be carried.** Any open req at any depth: `[ ]` or a flagged
  mark, Pending under an open owner. The owner may already be closed (the
  post-close card carries under a done task, as it drops there).
- **What is refused.** Dropped, carried, and hand-ticked `[x]` reqs get
  `split_not_splittable` with copy for that state (`Dropped — restore it
  first`, `Carried — open the linked task`, `Checked box — split an
  unchecked note instead`). An Excluded req gets `split_req_excluded`: `Its
  parent requirement was dropped — restore it first` (or `… was carried
  into its own task — it goes with it`).
- **The new task.** It is `- [ ] #task <text> [created:: today] ^<id>`. A
  trailing `^anchor` is reused as the ID. It goes **after the owner's
  block**, at the owner's indentation; a daily-note owner uses Task Split's
  daily-note rules (its `[root::]` note's `## Tasks`, else the inbox).
- **The branch travels whole.** Descendant reqs become the new task's reqs,
  and re-indentation rebases their depth. The block is laid out through
  `task_block_layout`: the back-link in the managed-link zone, and, when
  the branch brought descendant reqs, the req's direct notes filed into
  `NOTES` (created only when a note lands in it). A leaf req's notes stay
  loose notes of the new task.
- **The stub.** The req line becomes `- [>] [[#^id|Alias]]`, or the
  cross-note form, at the same position. The alias follows Task Split's
  rules.
- **The back-link.** The new task gets
  `🪢 LINKED FROM: [[Owner]] (💬 mentions)`, naming the real owner.
- **No inheritance and no dependency.** Nothing is inherited. Carry never
  adds a dependency to the old owner; `&` / Ctrl+Shift+B says that
  explicitly when wanted.
- **Overlaps.** Selecting an ancestor and one of its descendants together
  is refused (`split_overlap`). Carry the ancestor; the descendant travels
  with it.
- **Library entry.** `carry_req(snapshot, owner, row_key | line_digest)`
  carries one req named by its close-card row key (only the
  `/req/<path>~<8-hex>` suffix is matched) or by a line and its digest. It
  returns the created task's path, line, and block ID, the whole split
  plan, and a **move map** from old descendant lines (the owner's note,
  before) to new ones (the new task's note, after), which the close card's
  jump landing uses. A stale key or digest refuses as `split_stale_ref`, a
  line that is no req as `not_a_req`.

The SP32–SP50 cases in `tests/fixtures/task_split/vectors.json` pin carry
byte for byte (see [Task Split](task-split.md) §6).

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

`bob task rename-drawer REQUIREMENTS NOTES` (§3.5) retires the old name
everywhere (open, closed, and archived tasks, plus `^prj` `## Requirements`
sections, which become `## Notes`), preserving content byte for byte; drawer
bullets never become reqs. Where the owner already has `NOTES`, the two
merge, and heading links are repaired. One `bob task tidy` pass (§3.4)
then repairs open task blocks into canonical order. Both run as gated,
previewed, one-commit writes. (Shipped in the `drawer-rename`, `task-tidy`,
and `vault-migration` phases.)

## 10. Vectors

- **RQ** (`tests/fixtures/task_reqs/vectors.json`): the recognizer — owner
  and depth, every mark under open/done/cancelled owners, excluded
  descendants, the depth bound, anchors, whole-token `#task` matching, row
  keys, subtree ends, and note ranges.
- **RL** (`tests/fixtures/task_reqs/layout.json`): insertion slots,
  violations, tidy before/after, and note homes (§3.1–§3.2): Cancel Log,
  nested tasks, mixed indentation, drawers, and fenced lookalikes.
- **RW** (`tests/fixtures/task_reqs/writes.json`): drop, restore, toggle,
  insert, the first-req scoop, the `with_first_note` NOTES birth, CRLF notes,
  and every refusal code (§3.3).
- **SP32–SP50** (`tests/fixtures/task_split/vectors.json`): carry — `[>]`
  stubs, whole-branch moves with rebased depth, the NOTES rule on the new
  task, move maps, daily-note owners, and the carry refusals (§6).
- **RV** (`tests/fixtures/req_close/vectors.json`): close-card row building,
  verdict bytes, overlaps, staleness, jump landings, and copy strings.
  (Added in `close-engine`.)
