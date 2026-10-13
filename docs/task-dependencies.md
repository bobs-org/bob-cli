# Task dependency links

A task's prerequisites live as plain task dependency links on one
managed `⛓️ **DEPENDS ON:**` first-child line. That line is the source
of truth; the `[dependsOn::]` / `[id::]` fields are derived from it.
A Project Task (`^prj`) uses the same line. While `^prj` has its own
reason to be Blocked, every open task in its note is Blocked; that
inheritance, `#prj_is_blocked`, and blocked-link consumption are in
§13.

This file is the contract every implementation cites. The Rust side is
`src/native/task_dependencies/` (parser, formatter, link form, legacy
children) plus the reconciliation step in `bob task reconcile`
(formerly `bob task-status-hooks`, still accepted); the
JavaScript mirrors are the Depends-On grammar in bob-navigation-hotkeys,
the chip model in bob-ledger-tools, and the small recognisers in
task-status-cycler and block-id-prompt. Each implementation copies the
vector tables it needs from this file into its tests, citing the
section in a comment. If an implementation disagrees with a vector, fix
this doc first in the same change.

## 1. Vocabulary

- **Task Dependency Link** (aka **task dep link**, **dep link**): a
  plain, never transcluded Task Link to a prerequisite, placed on its
  dependent's Depends-On line, including a Project Task's.
- **Depends-On line**: the single managed first-child line that holds a
  task's dep links.
- **Dependent**: the task that owns the line.
- **Prerequisite**: a link target.
- **Blocked** stays the name of the derived `[?]` state.
- **Project dependency**, **project reasons**, **resident task**, and
  **consumption** are defined in §13. Ordinary tasks, including
  ordinary tasks inside project notes, keep the terms above.

## 2. Grammar

Example:

```markdown
- [?] #task Make appt w/ Rahway Hospital for CT scan! [dependsOn:: body__hospital-swarm,
  sase_bug_bash__e2e-sase-8v] ^rahway
  - ⛓️ **DEPENDS ON:** [[#^hospital-swarm]] • [[sase_bug_bash#^e2e-sase-8v]]
  - 🗓️ **SCHEDULE LOG**
```

### 2.1 Writer form

- `⛓️` (U+26D3 U+FE0F), a space, `**DEPENDS ON:**`, a space, then links
  joined by `•` (U+2022, with one space on each side).
- **No aliases, embeds, or strikes.**
- Existing links keep their order. New links are appended, so removing
  a link and adding it again moves it to the end. Never re-sort.
- **Position:** the first direct child, or the second if a
  `❌ **CANCEL LOG**` child exists.
  - Reuse the task's existing child indent; otherwise use the parent's
    indent plus one tab.
  - Preserve the note's line endings and its final-newline state.
  - On `^prj`, the machine-owned `🧩 **Sub-projects:**` line follows
    the Depends-On line (§13.2).
- **Empty list:** delete the line and remove the field. There is no
  placeholder.

### 2.2 Reader tolerance

Writers canonicalise all of these:

- the emoji is missing, or is the legacy `🔗`; VS16 is optional;
- the label is `DEPENDENCIES` instead of `DEPENDS ON`;
- the separator is `·`, `,`, or whitespace instead of `•`;
- a link is aliased, struck, or `!`-embedded;
- the line is a direct child other than the first.

### 2.3 Parse algorithm

1. Find the block links first (the shared link scanner, which already
   handles aliases, `!`, `~~`, and `[[#^id]]`), then check what
   surrounds them. Never split on separators, because an alias can
   contain one.
2. After removing the block links and the separators between them, the
   line must hold exactly the emoji (or nothing), the label, and
   whitespace. Anything else on the line makes it **malformed**.
3. Only a direct child of a `#task` line counts. Fenced code, nested
   lines, and blockquoted lines never count: a blockquoted task cannot
   own a Depends-On line (DP29; nav refuses dependency gestures there).

### 2.4 Recognisers that must reject the line

Dedicated Task Link and Pomodoro link recognisers, section titles, and
managed-log parsers in every repo must not claim a Depends-On line as
their own. The line is neither a Task Link bullet, nor a Pomodoro link,
nor a section title, nor a managed log (`🗓️ SCHEDULE LOG`,
`❌ CANCEL LOG`, Work Log entries).

## 3. Identity and link form

- **Prerequisite id.** Use the target's existing valid `[id::]` if it
  has one; it is never rewritten. Otherwise use the canonical
  `dependency_id(path, blockId)` (`note__path__blockid`), which the
  writer adds to the target. A target whose path cannot be encoded
  (spaces, dots) is refused **only when it has no `[id::]` yet**. Such
  a link is kept verbatim and warns (`unencodable_dependency_target`);
  it is never projected.
- **Link form.** Use the shortest form that is unambiguous under the
  hooks' resolver:
  1. `[[#^id]]` for a target in the same note;
  2. `[[basename#^id]]` when the basename is unique in the vault
     (case-insensitive);
  3. otherwise `[[dir/note#^id]]`, the full vault-relative path
     without `.md`. Links into `done/` always keep this explicit path
     form even when the basename is unique: basename links do not
     search `done/`, so the short form would stop resolving.
- **Field placement.** `[dependsOn:: a, b]` (comma-space) and `[id:: x]`
  go inside the Tasks suffix:
  - to the right of the freshness group
    (`[fresh::] [refresh::] [keeps::] [unblocked::]`), because both
    Rust Tasks parsers stop at the first unknown key, and those four
    are run-extending non-Tasks keys;
  - before a trailing `^block-id`;
  - in Tasks key order (`id` before `dependsOn`).
- **Field order.** The field mirrors the line's order.

## 4. Reconciliation (the hooks)

### 4.1 Scope and timing

- Only _open_ dependents are reconciled: status types Todo,
  In Progress, and On Hold, which includes `[?]`. Closed tasks are
  never touched.
- The step runs on the whole vault on every run, before edges and
  Blocked derivation, inside the existing guarded write.
- The previous daily note snapshot is never written. A target that
  lives in the previous daily note and has no `[id::]` keeps its link
  verbatim and warns (`previous_daily_target`); it is never stamped
  and never projected.

### 4.2 The dependency set

The dependency set is the line's links followed by any **legacy
children** (R8) not already on the line, deduplicated by (path,
block id). `^prj` uses this same table: R1–R10, with no R0, no D3,
and no R2 exception.

| # | Situation | Resolution |
| --- | --- | --- |
| R1 | Well-formed line | `[dependsOn::]` := the ids of the set's resolved task targets, in set order. Targets without `[id::]` get one. Field ids not accounted for are dropped, reported in the projection detail, and warned about (`dependency_field_ids_dropped`), except breadcrumbs (R4). |
| R2 | No line, field present | **Adopt** the field ids not covered by legacy children: write a canonical line linking to the tasks that carry each id and have a `^block-id`. Ids that can't be adopted stay in the field and are warned about (`unadoptable_dependency_id`). |
| R3 | A link doesn't resolve | **Heal** it when exactly one scanned task has both the link's block id and one of the dependent's unaccounted field ids. Rewrite the link to that task's location in the shortest form, then apply R1. |
| R4 | A link doesn't resolve and can't be healed | Keep it verbatim and warn (`unresolved_dependency_link`). It never blocks. While any unresolved link remains, keep the dependent's unaccounted field ids as **heal breadcrumbs**, so a task that is cut now and pasted later still heals. |
| R5 | Resolves to a non-task block | Keep it and warn (`non_task_dependency`). It is not projected. |
| R6 | Points to its own task | Not projected; warn (`self_dependency`). |
| R7 | Cycle | Keep it (every member stays Blocked) and warn (`dependency_cycle`) with the path. |
| R8 | **Legacy child**: a direct child bullet whose only content is one block link (plain, `![[…]]`, `~~[[…]]~~`, or `~~![[…]]~~`) and whose resolved target's id (its `[id::]`, canonical id, or same-note bare block id) is in the field | Counts as a dep link during the legacy window. The hooks never rewrite it; the migration converts it. Plain sole links count too, so un-embedding a legacy child with `!` doesn't silently drop the dependency. Any other sole embed (for example a `#^ref`) is content, not an edge. |
| R9 | Label but no links, and no legacy children | Delete the line and remove the field. |
| R10 | Malformed line | No change for that dependent; warn (`malformed_dependency_line`). Projection outputs count as structural, so notes modified less than 2 s ago are deferred by the quiet interval. |

### 4.3 Refinements beyond the table

- **Canonicalise.** A well-formed but non-canonical line is rewritten
  to the writer form with the same targets: label and emoji variants,
  other separators, `!`, `~~`, and aliases.
- **Archive targets.** A link that resolves into `done/` through the
  existing archive catalog is a resolved, _closed_ prerequisite. Its id
  is kept, it is never warned about, and it never blocks. Archive links
  keep the explicit `done/` path form from contract §3; it is the only
  form the hooks re-resolve, and it binds the JS canonicalisers too.
- **Removals.** Never infer a removal from a _missing_ line (R2 adopts
  instead). Do infer removals from a _present_ line.
- **No moves.** The hooks never move or reorder lines.
- **Output.** Report every change:
  - `dependency_projection_updates`, entries of kind
    `dependent_field`, `target_id`, or `line_removed`;
  - `adopted_dependency_lines`;
  - `healed_dependency_links`;
  - `canonicalized_dependency_lines`;
  - `legacy_dependency_children` (a count, for deciding when to retire
    legacy reading);
  - `dependency_warnings`, entries of `{kind, path, line, detail}`.
    Kinds are `unresolved_dependency_link`, `unadoptable_dependency_id`,
    `non_task_dependency`, `self_dependency`, `dependency_cycle`,
    `malformed_dependency_line`, `previous_daily_target`,
    `unencodable_dependency_target`, and `dependency_field_ids_dropped`.

## 5. Dependency semantics

- **Blocked.** A task is `[?]` while any prerequisite is open (AND,
  finish-to-start), while its `scheduled` date is in the future, or
  while it lives in a blocked project (§13). Done and Cancelled
  prerequisites stop blocking.
- **Promotion.** Pomodoro roots promote their prerequisites to
  Next/In Progress along the reconciled set: line links plus R8 legacy
  children, plus a resident's inherited `^prj` Depends-On links (§13).
  Strongest rank wins, the walk is cycle-safe, and recovery uses the
  same edges. Dependency promotion never raises a `^prj` target into a
  lane; a direct Task Link raises `^prj` to Next like any other task.
  The walk continues through it to that project's own Depends-On links.
  Lanes are sticky, and they do not survive Blocked.
- **Consumption.** A Blocked task keeps no live Task Link under
  today's open Pomodoros. A gesture that would create or keep such a
  link promotes the task's open prerequisites (own plus inherited)
  along those edges and then drops the link, except the own-schedule
  pull-forward in §13. `bob task reconcile` does the same for stale
  links.
- **Not edges:** `#^ref` embeds and archived targets.
- **Closing.** Closing a dependent **never** closes its prerequisites.
  Embedded-tree closes in capture `=x` and the cycler ignore
  Depends-On lines.
- **History.** Closed prerequisites stay on the line as history and
  show as muted chips. Prerequisites never prevent closing a dependent
  by hand.
- **Adding a dependency** sets the dependent to `[?]` if the target is
  open. On `^prj`, every open resident becomes `[?]` too. If the
  dependent was `[*]` or `[/]`, an open target rises to at least that
  lane (`getDependencyPromotionStatus`); a `^prj` target is never
  raised.
- **Removing a dependency** recovers the dependent to its derived rank
  _immediately_ when no open prerequisite, no future `scheduled` date,
  and no remaining project reason remain. Otherwise it stays `[?]`.
  Removing the last project reason recovers residents that have no
  own reason. The hooks keep the final word.
- **Closing.** Closing a planned prerequisite links the dependents it
  fully unblocked into its slot (§12).

## 6. The Depends on stage (navigation-hotkeys)

### 6.1 Entry points

| Cursor | Gesture | Edits |
| --- | --- | --- |
| A `#task` line | Ctrl+Shift+P then `b` on the Task Card (Blocked by) | this task; on `^prj`, the line plus resident side effects (§13) |
| Anywhere on a Depends-On line, including inside a link | Ctrl+Shift+P (skips the card and the property step) | the **owning** task, never the link under the cursor |
| A dedicated Task Link | Ctrl+Shift+P then `b` | the **linked** task in its own note, named in the title |
| A task line, counted | `N<Ctrl+Shift+P>` then `b` | this task plus the next N (existing add-to-all/remove-from-all, `k/n` badges, no Tab marks) |
| A chip | `＋` / hover `×` | opens the stage / removes that prerequisite |
| Anywhere | palette command **Edit task dependencies** (`edit-task-dependencies`, no default hotkey) | the task under the cursor |
| Prose with several links, or a selection spanning tasks | any | refused with a short reason |

Layout reuses the `bob-cnp` modal styling, with `CURRENT`,
`RESULTS`, and `BLOCKED` sections, a search row, and a footer of
`↑↓ navigate · ⇥ mark · ↵ toggle · esc dismiss`.

### 6.2 Pool

- Source: the Tasks plugin cache (`getTasks()` when `getState()` is
  `"Warm"`; Tasks 8.4.0 exposes `id` and `dependsOn` on each task).
  An authoritative Warm cache with zero tasks still counts as ready;
  open-buffer tasks participate and no vault fallback runs merely
  because the task array is empty.
- Open editor buffers override the cache for their notes, so unsaved
  edits count.
- If the cache isn't ready, the stage opens immediately from open
  buffers, then falls back to a vault scan that reads and prepares
  note snapshots in bounded chunks, yielding to the event loop
  between batches. The refresh is tied to a request generation:
  dismissal, leaving the dependency stage, or a newer request
  abandons obsolete work before painting, preserving the query,
  valid marks, and the complete parent set. Read failures are
  skipped without an unhandled rejection.
- Never read from disk on a keystroke; search and navigation perform
  no disk reads.

RESULTS rows include open tasks that pass the `#task` global filter,
including `ref/`, inbox, Blocked, and `#hide` (`#hide` tasks are muted
and ranked last). Excluded: daily notes (`YYYY/YYYYMMDD.md`), `done/`,
`_templates`, `_generated`, `_conflicts` (matched as a path segment),
dot-dirs, fenced code, and the dependent itself. CURRENT rows resolve
against everything, including closed, archived, hidden, and missing
targets, so any of them can still be removed.

### 6.3 Ranking

The stage ports `capture_link_tasks.rs::rank`:

- Every term must match. Each term scores its best tier across the
  cleaned description, `route:blockId`, block id, note route, and
  section heading: field prefix 3, word prefix 2, substring 1,
  in-order subsequence 0. Scores are summed.
- Ties keep canonical order: same note in document order, then
  In Progress, Next, Ready (by path, then line), then `#hide`.
- Blocked candidates render in their own BLOCKED section beneath
  RESULTS.
- BLOCKED badges name what blocks the candidate, counted from its own
  Depends-On line whether or not it carries a `^blockId`: `🔒 waits on N`
  only when N >= 1 open prerequisites remain (the stage never shows
  `waits on 0`); with no open prerequisite, a future
  `[scheduled:: YYYY-MM-DD]` reads `🔒 scheduled YYYY-MM-DD`, and otherwise
  the row reads `🔒 blocked`.
- An empty query shows CURRENT, then open tasks in the same note, then
  the In Progress and Next lanes.
- At most about 60 rows render. Typing reaches the rest.
- Matched characters are highlighted.
- The row badge shows the block id, never the path-encoded id.

### 6.4 Keys and guards

| Key | Action |
| --- | --- |
| type | fuzzy search |
| ↑ / ↓, ^N / ^P | move |
| ↵ | **toggle** the highlighted row (add if absent, remove if present) and close; with marks, apply every mark |
| ⇥ | mark or unmark the row (`＋ add` / `− remove` / `＋ id`) and move down |
| Esc | cancel; writes nothing and allocates no ids |

Guarded rows are disabled and show their reason: the dependent itself;
a cycle (`⟲`), checked on the graph _after_ the whole batch, with the
path in a tooltip (the graph includes implicit resident → `^prj`
edges, and trialing a `^prj` add also trials resident → target for
every open resident, §13); a `^prj` target inside the same project note
(`inside this project`); a target with no `[id::]` whose path can't be
encoded; a target or dependent that changed since the stage opened
(refuse, then reopen fresh). Removing a link is always allowed.

### 6.5 Block ids

A `＋ id` row opens the existing block-ID stage when the change is
applied. It is pre-filled from the prompt policy (`suggestPromptBlockId`
in both prompt-owning plugins): the raw target headline seeds a compact
slug (named phrases first, then the first three meaningful prose words,
accents normalized, at most 32 characters), with uniqueness checked
against the **target's** note (all blocks, plus pending batch reservations),
so ↵ accepts it. A valid, free legacy `[id::]` is reused as-is (for example
`flights`); an invalid or taken value — including a path-qualified value
such as `Tasks__target` — falls through to generation. Collisions walk
`stem-2`, `stem-3`, … past nine. A batch prompts for one target at a time.
Highlighting a row never writes. Typing replaces the suggestion and Escape
writes nothing. Project conversion's automatic IDs still use the existing
`suggestBlockIdFromTask`; capture and successor minting (§11.7, §12) are
unchanged.

### 6.6 One gesture writes

1. **Prepare cross-note targets first.** Add the `^id` and `[id::]`
   through the open editor if the note is open, otherwise with a
   preimage-checked `vault.process`.
2. **Commit the dependent's note** in **one editor transaction** (one
   Ctrl+Z): the line, the field, same-note target ids, status effects,
   folding of legacy children, and `[fresh:: today]` via
   `api.freshness.stampLine`. On `^prj` the same transaction also sets
   every open resident `[?]` with `#prj_is_blocked` when any added
   target is open, and transfers the strongest lane among `^prj` and
   its residents onto each open prerequisite (never onto a `^prj`
   target, §13). Nav never regroups lines between sections; reconcile
   does that within one interval.
3. **Consume Pomodoro links after a `^prj` block.** The live links of
   `^prj` and its newly Blocked residents are removed from today's open
   Pomodoros with `removeEmptiedEntries`, in the same write when the
   daily note is the edited note and in a follow-up write otherwise. A
   cleanup failure shows the `Pomodoro links not removed` chip and is
   never rolled back; reconcile repairs it.
4. **If preparation fails**, the dependent is untouched. An unused
   target id is acceptable; a link to an unprepared target is not.

Removing `^prj`'s last reason (the stage, Ctrl+D on the row, or api
v3) recovers `^prj` and every resident without its own remaining
reason through the gate in the same write: the tag is stripped and
`[unblocked::]` is written on Ready recoveries. Hand edits to `^prj`'s
Depends-On line keep the mirror's field behavior; resident side
effects of hand edits are reconcile's.

### 6.7 Notices

- `⛓ Now waits on "File for unemployment" · Blocked`
- `⛓ No longer waits on "Launch swarm…" · Ready again`
- `⛓ 2 added · 1 removed · Blocked (1 open)`
- `⛓ Project waits on "Fix LinkedIn now" · 2 tasks Blocked · removed 2 Pomodoro links`
- `⛓ Project no longer waits on "Fix LinkedIn now" · 2 tasks Ready again`

## 7. Chips (bob-ledger-tools)

```text
[?] Make appt w/ Rahway Hospital for CT scan!
    ⛓ depends on  ( ○ Launch swarm to find hospital )  ( ○ Run e2e on sase-8v ↗ sase_bug_bash )  ＋   waiting on 2
```

### 7.1 Chip anatomy

- a miniature checkbox showing the target's status symbol, coloured
  with the existing `--task-status-*` tokens (no new palette);
- the cleaned description, cut at about 40 characters;
- `↗ note` for a target in another note;
- a tooltip with the full text, note path, status name, and
  `scheduled` date.

### 7.2 States

| State | Look |
| --- | --- |
| Open, Next, In Progress, Blocked | coloured by status |
| Done | ✓, dimmed, struck text; more than three collapse to `✓×N` |
| Cancelled | muted ✕, visibly different from Done |
| Broken | dashed, `⚠ ^id not found` |
| Not a task | `⚠ not a task` |

**Summary:** `waiting on N` or `✓ all clear`. It is never written to
Markdown. `N` counts open prerequisites only: a broken link (DC7) or a
non-task block (DC8) never blocks per R4/R5 and never counts toward `N`,
so a Ready task with only such links reads `✓ all clear` beside its ⚠
chips. The ⚠ chip is the signal for those; the summary tracks blocking.

### 7.3 Interaction

- Click opens the target (Mod-click opens a new tab).
- Hover shows the native page preview.
- Hovering a chip shows a `×` that removes that prerequisite. A
  trailing `＋` opens the stage. Both go through nav api v1 and are
  hidden when it is absent.
- Putting the cursor on the line reveals the raw Markdown. Source mode
  stays raw.

### 7.4 Implementation requirements

- Chips never change line height.
- Only visible ranges are processed, after a cheap `DEPENDS ON`
  prefilter. Ownership walks ancestors with line lookups; the note is
  never copied wholesale.
- Reading rows follow the same owning-line rules as Live Preview: the
  row's Depends-On line is owned by a `#task` list item (its parent item
  is the task, per DP30), outside blockquotes, and the row parses as
  `accept`/`empty` (never chips on DP16, DP19, DP20, DP29, or DP31).
- Each rendered list item maps by order to its own line: the k-th `li`
  in the rendered section is the k-th list-item line in the section
  range, with fenced lines skipped. Chip actions carry that order-mapped
  0-based line (contract §9).
- Actions stay hidden when the item and line counts disagree, or when the
  mapped line is not the expected owned Depends-On line.
- Data comes only from the in-memory Tasks memo.
- `aria-label`s, visible focus, colour never the only signal, and
  reduced-motion support.
- Wraps between chips, never inside one, including at phone width.
- Without the plugin, the line still reads:
  `⛓️ DEPENDS ON: ^hospital-swarm • sase_bug_bash > ^e2e-sase-8v`.

## 8. Other gestures

| Gesture | Behaviour |
| --- | --- |
| `!` / `N!` | Only toggles transclusion. Refused on a Depends-On line, with a notice pointing to Ctrl+Shift+P. |
| Ctrl+Enter on a link on the line | Closes or reopens the **target** only: root-only, no strike, no re-embed, no tree close. Dependents recover immediately. |
| Cycler strike/restore when a target closes or reopens elsewhere | Skips Depends-On lines. |
| Alt+] / Alt+[ on the line | Cycles the target of the link under the cursor. Never reformats the bullet. |
| Ctrl+Shift+Enter on the line | Refused, with a notice pointing to Ctrl+Shift+P. |
| Ctrl+D on the Depends on row | Deletes the line, the field, and any legacy children, with immediate recovery. |
| Hand edits in Obsidian | Once the cursor leaves the edited line (short debounce), nav applies R1/R9 to that task. Deleting the whole line clears the field. Malformed lines are left alone. |
| `bob capture` `&note:id` (and the Bob Mac Capture `&` picker) | Adds prerequisites to a new task or an explicit `@note+id` dependent through the staged capture writer: new links append in typed order (repeats are no-ops), legacy children fold and field-only dependencies are adopted, fields are derived, and §5 Blocked/promotion applies in the same batch. On `^prj`, `&` writes the Depends-On line and applies resident side effects (§13). It never removes or reorders links; removal stays with Ctrl+Shift+P and Ctrl+D. See `docs/capture.md`. |
| Ctrl+Shift+M, `task archive` | The line moves with its task. Same-note links inside a moved block whose target stayed behind gain the source note path. Moving open tasks **into** a blocked project sets them `[?]`, tags them `#prj_is_blocked`, promotes verifiably open prerequisites to the strongest moved lane, and removes their live links from today's open Pomodoros in the move write. Moving them **out** strips the tag and recovers those without own reasons, writing `[unblocked::]` when [freshness.md §5](freshness.md#5-who-stamps) says to. Closed lines are never edited. |
| Ctrl+Alt+Shift+N (`create-project-note-from-task`) | Forward: the source's Depends-On line becomes the first child of `^prj`, and its inline `scheduled` stays on `^prj` (rebase same-note links to the source note; never seed that line as a Tasks row; skip the machine-owned Sub-projects child); the project lands blocked only for a carried open prerequisite or a future inline date. Reverse: the line becomes the restored task's first child and the date stays inline on its root task (links re-formed relative to the destination), with `#^prj` links repointed before Pomodoro cleanup so live links are consumed. `[?]` sources are accepted; blocked residents are tagged/recovered through the shared gate and live links are consumed when they enter a blocked project. Details in §13. |
| "Rewrite dependency navigation links" command, `migrate-dependency-bullets.mjs` | Deleted. They emit embeds. |

## 9. Plugin api v3 (navigation-hotkeys)

```js
app.plugins.plugins["bob-navigation-hotkeys"].api = Object.freeze({
  version: 3,
  openDependencyStage(ref),            // ref: { path, line } — any line of the task block or its Depends-On line
  removeDependency(parentRef, target), // target: { path, blockId }
  claimReviewWalkCompletion(editor),   // task-status-cycler's landed checklist Ctrl+Enter hook
  reviewWalk: Object.freeze({
    version: 1,
    capture(editor),          // sync, never throws: null (not on a landing) | { busy: true } | frozen origin
    continue(origin, outcome), // async, never throws: resolves { ok, advanced, stopped }
  }),
  notice: Object.freeze({
    version: 1,
    showUnblocked(model),     // additive: renders the Unblocked notice card (§12.6); returns true when shown
  }),
});
```

- `openDependencyStage` and `removeDependency` return Promises resolving
  to `{ ok, reason? }` and never throw.
- `ref.line` and `parentRef.line` are 0-based line indexes into the
  note's lines (the same indexing the editor and `findOwningTaskLine`
  use): the Depends-On line itself, or any line of the owning task
  block. Non-integer or out-of-range lines refuse with `invalid-ref` /
  `line-out-of-range`.
- `removeDependency` re-reads the dependent and refuses with a notice
  when it is stale, and refuses with `not-on-line` (never a silent `ok`)
  when the target is not on the dependent's line.
- `claimReviewWalkCompletion(editor)` is task-status-cycler's Ctrl+Enter
  hook. It synchronously returns `null` unless the cursor is on the PRE/POST
  row the review walk just landed on; otherwise it returns a Promise of
  `{ ok, reason? }` and never throws. See [freshness.md §6](freshness.md#6-review-ritual).
- `reviewWalk.capture(editor)` returns `null` when the cursor is not on the
  row the walk just landed on (callers behave normally), `{ busy: true }`
  while a review gesture is in flight or settling (callers swallow the key
  and write nothing), and otherwise a frozen origin holding the gesture
  lock. `reviewWalk.continue(origin, outcome)` advances the walk exactly
  once when the origin is current and the outcome resolves the row, shows
  `outcome.notice` exactly once either way, and settles the lock. An
  `outcome` of `null` settles without advancing. Every captured origin is
  settled exactly once on every path. Callers feature-detect
  `api.version >= 3` with `reviewWalk.version >= 1` and keep today's
  behavior otherwise.
- `notice.showUnblocked(model)` renders the shared successor-link notice
  model (§12.5) as the Unblocked card (§12.6) and returns `true` when
  shown. The member is additive: the api stays `version: 3` and callers
  feature-detect `api.notice?.version >= 1`, falling back to a plain
  text notice when it is absent.
- Plugins never import each other's `main.js`. bob-ledger-tools
  feature-detects `api?.version >= 1`.
- Additive `inboxRoute` v1 namespace (nav 2.9.0+; `api.version` stays 3,
  consumers feature-detect `api.inboxRoute?.version >= 1`):
  `isInboxNote(path)` (sync, never throws: boolean),
  `prompt(request)` (Promise of `{ kind: "move", path, name } |
  { kind: "stay" } | { kind: "cancel" }`, never rejects),
  `commit(request)` (Promise of
  `{ ok, name, count, notice, handledRefs, reason? }`, never rejects),
  where `request` carries
  `{ editor, path, line, additionalTaskCount?, actionLabel, expected?,
  reservedBlockIds?, destinationPath? }`. With a null plugin (after
  unload), `isInboxNote` returns false, `prompt` resolves
  `{ kind: "stay" }`, and `commit` resolves
  `{ ok: false, reason: "unavailable" }`. block-id-prompt's
  Ctrl+Shift+Enter routes through this namespace and falls back to
  today's behavior when it is absent.

## 10. Legacy window and rollout

- **Legacy readers.** Rust and nav read R8 legacy children for one
  release.
- **Touch migrates.** Any nav dependency write folds the dependent's
  legacy children into its line in the same transaction.
- **Chips and compat ship before any writer emits lines.** That is why
  `nav-model` depends on `chips` and `compat`.
- **Fleet before migration.** `fleet-rollout` installs everywhere
  before `vault-migrate`.
- **The MacBook.** Its cron is the only hooks runner. Until it is
  updated, it keeps Blocked correct from the fields but loses
  dependency promotion for parents that already have a line.
  Project-blocking rollout is in §13.11.

## 11. Conformance vectors

Implementations copy the tables they need into their tests, citing the
section in a comment. `line` below is the candidate Depends-On line on
its own; `context` names what surrounds it. `verdict` is one of
`accept(n)` (a line with n targets), `empty` (label but no links),
`malformed`, or `not-a-line`. Project-dependency vectors (PDR, PDB,
PDL, PDG, PDC, PDF, PDT) live in §13. Unblock (`[unblocked::]`)
vectors (UB) live in [freshness.md §10b](freshness.md#10b-ub--unblocked-vectors).

### 11.1 DP — parse vectors

| # | line | context | verdict |
| --- | --- | --- | --- |
| DP1 | `⛓️ **DEPENDS ON:** [[#^hospital-swarm]]` | first direct child of a `#task` | accept(1) |
| DP2 | `⛓️ **DEPENDS ON:** [[cash#^unemployment]]` | first direct child | accept(1) |
| DP3 | `⛓️ **DEPENDS ON:** [[money/cash#^unemployment]]` | first direct child, `cash.md` exists in two dirs | accept(1) |
| DP4 | `⛓️ **DEPENDS ON:** [[#^a]] • [[#^b]]` | first direct child | accept(2) |
| DP5 | `⛓️ **DEPENDS ON:** [[#^a\\|b • c]]` | alias contains `•`; never split on separators | accept(1) |
| DP6 | `⛓️ **DEPENDS ON:** ~~[[#^a]]~~` | first direct child | accept(1), canonicalise |
| DP7 | `⛓️ **DEPENDS ON:** ![[#^a]]` | first direct child | accept(1), canonicalise |
| DP8 | `🔗 **DEPENDS ON:** [[#^a]]` | legacy emoji | accept(1), canonicalise |
| DP9 | `**DEPENDS ON:** [[#^a]]` | emoji missing | accept(1), canonicalise |
| DP10 | `⛓️ **DEPENDENCIES:** [[#^a]]` | legacy label | accept(1), canonicalise |
| DP11 | `⛓ **DEPENDS ON:** [[#^a]]` | missing VS16 | accept(1), canonicalise |
| DP12 | `⛓️ **DEPENDS ON:** [[#^a]] · [[#^b]]` | `·` separator | accept(2), canonicalise |
| DP13 | `⛓️ **DEPENDS ON:** [[#^a]], [[#^b]]` | `,` separator | accept(2), canonicalise |
| DP14 | `⛓️ **DEPENDS ON:** [[#^a]] [[#^b]]` | whitespace separator | accept(2), canonicalise |
| DP15 | `⛓️ **DEPENDS ON:** [[#^a` | half-typed link | malformed |
| DP16 | `⛓️ **DEPENDS ON:** [[#^a]] needs review` | trailing prose | malformed |
| DP17 | `⛓️ **DEPENDS ON:**` | label only | empty (R9) |
| DP18 | `⛓️ **DEPENDS ON:** [[#^a]]` | inside fenced code | not-a-line |
| DP19 | `⛓️ **DEPENDS ON:** [[#^a]]` | grandchild (nested two levels) | not-a-line |
| DP20 | `⛓️ **DEPENDS ON:** [[#^a]]` | the line itself hangs directly off a Work Log entry (its parent is the entry, not a `#task`) | not-a-line |
| DP21 | `⛓️ **DEPENDS ON:** [[#^a]]` | third direct child, after two prose children | accept(1) |
| DP22 | `⛓️ **DEPENDS ON:** [[#^a\\|swarm]]` | aliased link | accept(1), canonicalise |
| DP23 | `⛓️ **DEPENDS ON:** [[note]]` | bare note link, no block id | malformed |
| DP24 | `🔗️ **DEPENDS ON:** [[#^a]]` | link emoji with VS16 | not-a-line |
| DP25 | `⛓️ **DEPENDS ON:** [[note#Heading]]` | heading link, no block id | malformed |
| DP26 | `⛓️ **DEPENDS ON:** • ,` | label followed only by separators | malformed |
| DP27 | `⛓️ **depends on:** [[#^a]]` | lowercase label | not-a-line |
| DP28 | `⛓️ **DEPENDS ON:** [[#^a]]` | no list marker | not-a-line |
| DP29 | `> - ⛓️ **DEPENDS ON:** [[#^a]]` | blockquoted line: every recogniser rejects it and no writer round-trips it | not-a-line |
| DP30 | `⛓️ **DEPENDS ON:** [[#^a]]` | first direct child of a `#task` which is itself nested under a Work Log entry (unlike DP20, the line's owner is a real task) | accept(1) |
| DP31 | `⛓️ **DEPENDS ON:** needs review` | prose-only line: the label is followed by prose with no link | malformed |

### 11.2 DW — write vectors

`→` separates before and after. `field` is the task line's
`[dependsOn::]` value (`—` means absent).

| # | operation | before | after |
| --- | --- | --- | --- |
| DW1 | create as first child | task with a `🗓️ **SCHEDULE LOG**` child, no line, field `—` | line inserted before the log child; field set |
| DW2 | create after Cancel Log | task with `❌ **CANCEL LOG**` then a prose child | line inserted second, after the Cancel Log |
| DW3 | append | line `[[#^a]]`, field `id-a` | line `[[#^a]] • [[#^b]]`, field `id-a, id-b` |
| DW4 | remove the middle link | line `a • b • c` | line `a • c`, field drops `id-b` |
| DW5 | remove the last link | line `[[#^a]]`, field `id-a` | line deleted, field removed |
| DW6 | re-add moves to the end | line `a • b`, re-add `a` | line `b • a`, field `id-b, id-a` |
| DW7 | field order mirrors the line | line `b • a` | field `id-b, id-a` |
| DW8 | tab indent | parent indented with a tab | child line uses one deeper tab |
| DW9 | space indent | parent indented with two spaces | child line reuses the task's existing child indent |
| DW10 | CRLF preserved | note uses CRLF | inserted line uses CRLF |
| DW11 | no final newline preserved | note has no trailing newline | edit adds none |
| DW12 | same-note link form | target in the same note | `[[#^block-id]]` |
| DW13 | unique basename link form | target in `cash.md`, unique in the vault | `[[cash#^block-id]]`, no path |
| DW14 | ambiguous basename link form | `cash.md` exists in two dirs | `[[money/cash#^block-id]]` |
| DW15 | existing `[id::]` preferred | target has `[id:: custom]` and an encodable path | field uses `custom`, never rewritten |
| DW16 | unencodable path refused | target path has a space, target has no `[id::]` | write refused with a reason |
| DW17 | field placement | task line has `[fresh:: 2026-10-02]` and `^task-id` | `[dependsOn:: …]` lands right of `fresh`, before `^task-id`, `id` before `dependsOn` |
| DW18 | fold legacy children | dependent has a line plus a legacy `![[…]]` child | one transaction folds the child into the line |
| DW19 | canonicalise variants | any of DP6–DP14 | writer form with the same targets |

### 11.3 DR — reconcile vectors

Each vector names the rule under test. Vectors marked `warn` also
produce a `dependency_warnings` entry of the named kind.

| # | rule | situation | outcome |
| --- | --- | --- | --- |
| DR1 | R1 | well-formed line, two resolved open targets | field set to both ids in line order; warn-free |
| DR2 | R1 | well-formed line, one target missing `[id::]` | target gains `[id::]`; field set (`target_id`) |
| DR3 | R1 | well-formed line, field holds a stale third id | stale id dropped (`dependency_field_ids_dropped`), warn-free otherwise |
| DR4 | R2 | no line, field with two adoptable ids | canonical line written (`adopted_dependency_lines`) |
| DR5 | R2 | no line, field holds an id with no `^block-id` anywhere | id stays in the field; warn `unadoptable_dependency_id` |
| DR6 | R3 | link `[[old#^x]]` unresolvable; exactly one scanned task has block id `x` and an unaccounted field id | link rewritten to the shortest form (`healed_dependency_links`), then R1 |
| DR7 | R4 | link unresolvable and unhealable | link kept verbatim, never blocks; warn `unresolved_dependency_link` |
| DR8 | R4 | DR7's note, second run with the target still absent | unaccounted field ids kept as heal breadcrumbs |
| DR9 | R4 | DR8's note after the target is pasted back | link heals per R3 |
| DR10 | R5 | link resolves to a non-task block | link kept, not projected; warn `non_task_dependency` |
| DR11 | R6 | link points to its own task | not projected; warn `self_dependency` |
| DR12 | R7 | two tasks link to each other | both stay Blocked; warn `dependency_cycle` with the path |
| DR13 | R8 | direct child `![[cash#^unemployment]]`, id in the field | counts as a dep link; child never rewritten |
| DR14 | R8 | direct child plain `[[cash#^unemployment]]` (un-embedded), id in the field | still counts as a dep link |
| DR15 | R8 | direct child `![[#^ref]]` reading embed | content, not an edge |
| DR16 | R9 | label-only line, no legacy children | line deleted, field removed (`line_removed`) |
| DR17 | R10 | half-typed line (DP15) | no change; warn `malformed_dependency_line` |
| DR18 | scope | closed (`[x]`) dependent with a stale line | untouched |
| DR19 | archive | link resolves into `done/` | id kept, never warned, never blocks |
| DR20 | R2+R8 | field-only dependent with a legacy child covering one of two field ids | adoption writes a line for the uncovered id only |
| DR21 | edges | `#^ref` embed under a Pomodoro root | not a promotion edge |
| DR22 | idempotence | second run over a reconciled vault | zero changes |

### 11.4 DK — ranking vectors

Abstract candidates, in canonical order:

| key | text | route | blockId | section |
| --- | --- | --- | --- | --- |
| A | File for unemployment | cash | unemployment | Money |
| B | Call unemployment office | cash | — | Money |
| C | Dispute North Face jacket | cash | — | — |
| D | Launch swarm to find hospital | body | hospital-swarm | Health |
| E | Run e2e on sase-8v | sase_bug_bash | e2e-sase-8v | Bugs |

Each query's terms all match (AND); a term scores its best tier over
description, `route:blockId`, block id, route, and section (3 / 2 / 1 /
0); scores sum; ties keep canonical order; an empty query keeps
canonical order. `rank` in `src/native/capture_link_tasks.rs` runs
these vectors verbatim.

| # | query | result | exercises |
| --- | --- | --- | --- |
| DK1 | `unemp` | A, B | field prefix (3) outranks word prefix (2) |
| DK2 | `cash:un` | A | `route:blockId` field prefix |
| DK3 | `face` | C | word prefix (2) |
| DK4 | `pit` | D | substring (1) inside `hospital` |
| DK5 | `uof` | B, C, D | in-order subsequence (0); ties keep canonical order |
| DK6 | `e2e` | E | block-id field prefix (3) |
| DK7 | `cash unemp` | A, B | AND across terms; 3+3 outranks 3+2 |
| DK8 | _(empty)_ | A, B, C, D, E | empty query keeps canonical order |

### 11.5 DC — chip model vectors

`lookup` maps `(path, blockId)` to a task or to nothing. `noteLabel`
is `↗ note` for a cross-note target, else absent. Text cuts at about
40 characters.

| # | target | chips | summary |
| --- | --- | --- | --- |
| DC1 | open Todo | `○` coloured Todo, full text | `waiting on 1` |
| DC2 | Next | `*` coloured Next | `waiting on 1` |
| DC3 | In Progress | `/` coloured In Progress | `waiting on 1` |
| DC4 | Blocked | `?` coloured Blocked | `waiting on 1` |
| DC5 | Done | `✓`, dimmed, struck text | `✓ all clear` (with DC1 also present: `waiting on 1`) |
| DC6 | Cancelled | muted `✕`, visibly different from Done | per remaining open count |
| DC7 | missing `(path, blockId)` | dashed, `⚠ ^id not found` | never blocks, never counts as waiting |
| DC8 | resolves to a non-task block | `⚠ not a task` | never blocks, never counts as waiting |
| DC9 | cross-note target | chip plus `↗ sase_bug_bash` | `waiting on N` |
| DC10 | four Done targets | `✓×4` collapsed | `✓ all clear` |
| DC11 | no open targets, one Done | `✓` chip | `✓ all clear` |
| DC12 | 60-character description | text cut at about 40 characters, full text in the tooltip | `waiting on 1` |

### 11.6 SL — successor-link vectors

Outcomes of the shared successor-link rule (§12). Rust tests run them
through capture (`!` and close); cycler tests run them through the pure
helpers.

| # | Situation | Outcome |
| --- | --- | --- |
| SL1 | P linked in the running entry; D `[?]` depends only on P | link right after P's bullet subtree; D `[?]`→`[*]`; `unblocked_by: [P]` |
| SL2 | P linked in queued FIX while BOB runs; `!` retirement moves P's struck link into BOB | D at P's vacated FIX position; FIX not removed as empty |
| SL3 | D also depends on open Q | no link; `still_blocked{waits_on, 1}`; D stays `[?]` |
| SL4 | D's only remaining blocker is a future `scheduled` | no link; `still_blocked{scheduled}`; schedule untouched |
| SL5 | D already has a live link under another open entry | no link (`already_planned`); `[?]`→`[*]`; existing link not moved |
| SL6 | D's only links today are struck or under closed entries | linked as in SL1 |
| SL7 | `=!` closes BOB; nothing carried; FIX follows | `- [ ] () — BOB` created right after BOB holding only D; `entry_created`, `next_up`; `next_pomodoro` names it |
| SL8 | `=x` close with carried links; an embed P completes | D appended after the carried lines in the continuation |
| SL9 | P has no live link today | D `[?]`→`[ ]`; `not_planned_today`; successor logic leaves the ledger alone |
| SL10 | D has no `^block-id` | ` ^<mint>` appended (SB rule); `block_id_created: true`; `block_id` is the mint |
| SL11 | D is `^prj` / carries `#hide` | recover only; `project_task` / `hidden` (except §12.10) |
| SL12 | One gesture closes P1 and P2; D depends on both | D linked once, after the earlier anchor; `unblocked_by: [P1, P2]` |
| SL13 | Chain P → D → E | D linked; E untouched and not reported |
| SL14 | D's basename is ambiguous | `[[dir/note#^id]]` |
| SL15 | D is a stale `[ ]` whose only prerequisite was P | linked; `[ ]`→`[*]` |
| SL16 | No task in C carries `[id::]` | gate: no lookup, no extra reads; empty arrays; `checked` |
| SL17 | Six successors from one gesture | none linked; all `breaker`, recovered to derived rank |
| SL18 | `plan.link_unblocked: false` | recovery to derived rank; no links or mints; `disabled` |
| SL19 | The same close re-applied (already Done) / a double press | no writes, no rows |
| SL20 | Draft `!P` then `!D` | the first item's `unblocked` omits D (net) |
| SL21 | Cancel, reopen, raw checkbox, hooks run, `=*` park | never link |
| SL22 | Ctrl+Enter on a Depends-On line | unchanged (nothing) |
| SL23 | P closes as a subtask of an embedded root R planned today | P anchors at R's link (`inherit`) |
| SL24 | Close creates no continuation; a later open entry named BOB exists | D appended to that BOB entry; `entry_created: false` |
| SL25 | D already `[*]` and already planned | nothing written, no row |
| SL26 | D lives in an inbox file | linked; `inbox: true` |
| SL27 | `=x~K` drops D's link in the same close that completes P | D not re-linked (`already_planned`); recovers to `[ ]` |
| SL28 | D lives in today's day file | link names the note (`[[20261009#^id]]`), never `[[#^id]]` |

### 11.7 SB — successor block-ID vectors

Each row feeds the raw task line through the successor mint pipeline —
the body after the status box, cleaned with the default `#task` global
filter — then through `mint_block_id` with the note's used IDs
(`src/native/capture_block_ids.rs`,
`successor_mints_follow_the_pinned_vectors`).

| # | raw task line | used IDs in the note | mint |
| --- | --- | --- | --- |
| SB1 | `- [?] #task Renew the library books` | — | `renew-library-books` |
| SB2 | `- [?] #task Fix apollo machine` | — | `fix-apollo-machine` |
| SB3 | ``- [?] #task Add support for new `%hold` directive`` | — | `hold` |
| SB4 | `- [?] #task Review [[sase#^fix-apollo\|apollo fix]] notes` | — | `apollo-fix` |
| SB5 | `- [?] #task Book flights [scheduled:: 2026-10-13] #task` | — | `book-flights` |
| SB6 | `- [?] #task Book flights [scheduled:: 2026-10-13] #task` | `book-flights` | `book-flights-2` |
| SB7 | `- [?] #task Fix it` | `fix`, `fix-2` … `fix-9` | `task` |
| SB8 | `- [?] #task é🚀` | — | `task` |
| SB9 | `- [?] #task é🚀` | `task`, `task-2` | `task-3` |

SB1 is plain prose. SB2 drops the leading verb into the second
suggestion (`apollo-machine`) and mints the first. SB3 mints the
backticked phrase. SB4 mints the wikilink alias, not the target. SB5
strips the inline field and both `#task` tags. SB6 takes the `-2`
suffix on collision. SB7 falls back to `task` when every suggestion and
every `-2`…`-9` suffix is taken. SB8 falls back on non-ASCII text with
no words. SB9 walks the `task-N` fallback past taken IDs.

## 12. Successor links

When a Bob close gesture completes a task planned in today's ledger,
every direct dependent that this close fully unblocked is linked into
the predecessor's slot and becomes Next in the same write. When that
close also empties the predecessor's project note, the project's `^prj`
task is pulled in the same way and loses `#hide` (§12.10).

- **Gesture.** One Obsidian Ctrl+Enter, or one capture **item**. A
  multi-item draft is several gestures, planned in order against the
  staged batch.
- **Predecessor (P).** A task this gesture moved from open to Done. That
  covers the root and every embedded subtask closed with it.
- **Successor (D).** An open direct dependent of a predecessor whose
  **last** blocker this gesture removed.
- **Anchor.** Where a predecessor was planned today. It is read from
  today's day text **before** this gesture strikes or moves anything.
- **Successor link.** The plain Task Link bullet the gesture inserts for
  D, for example `\t- [[sase#^relaunch-agents]]`. It is never an embed
  and never annotated.
- **Live link.** An unstruck Task Link sub-bullet (plain, embed, or
  `#`-deferred; 🍅 markers ignored) under an **open** `[ ]` entry of
  today's day file's `## Pomodoros` section. Struck links and links
  under closed entries are history, not plans.

### 12.1 Trigger

Successor linking runs inside these gestures, and only when they
**complete** a task:

| Gesture | Where |
| --- | --- |
| Ctrl+Enter on a Task Link (plain or embed) under any Pomodoro, on the task line in its own note, or through the fallback link branch | task-status-cycler |
| Ctrl+Enter on a Pomodoro line (its closed embeds) | task-status-cycler |
| nav's PRE/POST walk Ctrl+Enter (`completeTaskAtCursor`) | task-status-cycler + nav |
| `bob capture '!note:id'` | bob-cli |
| `bob capture` closes that complete links: `=x` with embeds, `=x!M`, `=!`, `^route:id=x…` | bob-cli |
| Alt+] / Alt+[ status cycling into Done | task-status-cycler |

The same complete gestures trigger project pull-in (§12.10).

It **never** runs on any of the following:

- cancel (the nav Ctrl+Shift+P cancel, Alt+[ into Cancelled);
- reopen;
- raw checkbox clicks;
- `bob task reconcile` / hooks;
- `=*` parks;
- Ctrl+Enter on a Depends-On line.

Cancels still recover dependents exactly as today. Cancel never links
a successor or a pull-in; it still un-hides an emptied project's
`^prj` (surfaces only, §12.10).

### 12.2 Rule

One definition, two engines (Rust capture and the JS cycler):

```text
C        = tasks this gesture moved open → Done (roots + closed embedded subtasks),
           read from the staged post-close text
GATE     : if no c ∈ C carries an [id::] field → skip the dependents lookup and extra
           vault reads. unblocked_check = "checked". Project pull-in (§12.10) still
           runs from the staged text of C's notes.
match(c) = c's [id::] value

ANCHORS (computed on today's day text BEFORE this gesture's strikes, moves, or retirement,
         but AFTER any explicit link step of the same item, e.g. ^route:id=x…)
  closing    : c's link sits under the Pomodoro this gesture closes
  slot(E, b) : else the first live link to c in ledger order, under open entry E
               (b = that bullet)
  inherit    : a closed subtask with no live link of its own takes its root's anchor
  none       : otherwise (no today day file, no ## Pomodoros, or not planned today)

CANDIDATES = open tasks D (status type Todo/InProgress/OnHold: ' ', '?', '*', '/'),
             outside done/, whose [dependsOn::] contains match(c) for some c ∈ C

FOR EACH D, in this order:
  post_open = D's open prerequisites in the staged post-close snapshot
              (exactly task_dependency_states)
  1. post_open ≠ ∅                → still_blocked{reason: waits_on, waits_on: |post_open|}
  2. D.scheduled > today          → still_blocked{reason: scheduled, scheduled}
  3. D is a resident of a still-blocked project
                                  → still_blocked{reason: project}  (PDC1; D stays [?])
  4. D's block id is prj          → recover; not_linked: project_task
                                    (except §12.10: a ^prj of an emptied
                                    project is a pull-in, not project_task)
  5. D carries #hide              → recover; not_linked: hidden
  6. D had a live link before this gesture
                                  → recover; not_linked: already_planned
  7. no predecessor of D in C has an anchor
                                  → recover; not_linked: not_planned_today
  8. plan.link_unblocked is false → recover; not_linked: disabled
  9. otherwise                    → SUCCESSOR, anchored at the earliest (ledger order)
                                    anchor among D's predecessors in C

PROJECT RECOVERY: when a `^prj` recovers (step 4), every resident of
           that project loses `#prj_is_blocked`. Residents with no
           remaining own reason recover too (recover-only, never
           successor-linked) and receive `[unblocked::]` when
           freshness.md §5's WRITE rule applies. Those recoveries
           do not count toward the breaker (PDC2). A resident that
           still has an open prerequisite of its own stays Blocked
           (PDC3).
BREAKER  : more than 5 successors in one gesture → link none; each becomes
           recover + not_linked: breaker
ORDER    : successors by anchor ledger position, then note path, then line
recover  : '?' → derived rank on the POST-gesture day text: '*' if D has a live link
           there, else ' '. ' ', '*', '/' are unchanged.
SUCCESSOR: '?' or ' ' → '*'; '*' and '/' are unchanged. Mint a missing ^block-id, then
           insert the successor link (Placement).
```

- **Identity.** Matching uses only c's `[id::]` value. That is the one
  identity under which `task_dependency_states` resolves blocking: its
  identity map is built from explicit `[id::]` fields. A dependent that
  names only the canonical `note__block-id` of a target **without**
  `[id::]` was never blocked by that target under the derived rule, so
  it cannot be "unblocked" by closing it. The hooks heal that rare stale
  case on their next run, because they write the target's `[id::]`.
  - This drops the canonical-id match the Rust `!` recovery uses today.
    The JS cycler already matches only `[id::]` values.
  - The gate is therefore exact: about 96% of open tasks carry no
    `[id::]`, and closing one of them can never unblock anyone.
- **Graph transition, not checkbox.** Eligibility depends on `post_open`
  and on D depending on something this gesture closed. Every c in C was
  open before the gesture, so "pre_open ∩ C ≠ ∅" holds automatically. A
  stale `[ ]` dependent therefore qualifies (SL15), and a `[?]` with
  another open prerequisite never does.
- **The already-planned baseline** is the day text **before** the
  gesture. Three consequences:
  - A dependent whose link this same close carries forward stays planned
    and is not duplicated.
  - A dependent whose link this same close **drops** (`=x~K`) is never
    re-linked. The drop wins, and the dependent recovers to Ready.
  - The recovered rank is derived from the **post**-gesture text,
    because that is what the hooks will see.
- **Rows report what the gesture did.** A dependent appears in
  `unblocked[]` only if this gesture changed its status or linked it. An
  already-Next dependent that is already planned produces no row.
  `still_blocked[]` lists every open dependent of C that stays blocked.
- **No recursion.** Linking D is not completing D, so D's own dependents
  wait (SL13).
- **No managed-line guard in v1.** The field is the single identity,
  exactly as recovery and the hooks use it. Staleness is the same window
  Blocked derivation already has.

### 12.3 Anchors and placement

The inserted line is always `<indent>- <link>`. The indent is the anchor
bullet's indent for slot anchors, and one tab for entry children.
Nothing else goes on the line: prose would stop it counting as a Task
Link.

**Slot anchors** (`slot(E, b)`): insert immediately after bullet `b`'s
subtree (`b` plus its deeper-indented children), in successor order.

- **Rust (`!`).** Insert **before** retirement runs. Retirement then
  strikes `b` in place, or moves `b`'s subtree into the running entry,
  while the successor stays at the vacated position in E. Because E
  still has a child, it is never removed as an emptied placeholder
  (SL2).
- **Obsidian.** Insert before the cycler strikes the cursor link. The
  insertion is below the cursor line, so the strike target is
  unaffected.

**Closing anchors.** The target entry is the first that applies:

1. the continuation this close created (`next_pomodoro.created`);
2. else the first open entry **after** the closed one with the same
   non-empty name;
3. else a new placeholder, `- [ ] () — NAME` (or `- [ ] ()` when the
   closed entry was unnamed). It goes immediately after the closed
   entry's sub-bullet range, exactly where the close would have created
   its continuation. Successors count as carried.

Successors are appended after the target's existing children; a lone
`\t- ` stub is replaced. `link.entry_created` is true when the entry
did not exist before this gesture, in case 1 or 3.

**Next up.** `link.next_up` is true when, after the gesture, the target
entry is the first open untimed placeholder in the ledger: the one a
bare `=` would start. A created continuation becomes next up, exactly
like carried work today. The notice always says so, so it is never a
surprise.

### 12.4 Writes

Each gesture makes one write set: one staged batch in capture, one
planned pass in Obsidian.

- **Successor's note.** Its status changes per the rule. A missing
  `^block-id` is appended as ` ^<mint>` at the end of the task line.
  Removing whole-token `#hide` from a pulled-in `^prj` is the one
  non-status line edit (§12.10). `#hidden` and `#hideaway` stay.
- **Day file.** Successor bullets are inserted, plus the created
  placeholder when one is needed.
- **Freshness.** Never touched: no `[fresh::]` stamp, and an existing
  stamp is kept byte-for-byte. Automation never stamps. An unstamped
  successor surfaces in tomorrow's NEXT review, which is correct.
- **Schedule.** Never touched: no `scheduled` change and no Schedule Log
  line. Eligible successors have no future date by definition.
- **Fields.** No `[id::]` / `[dependsOn::]` writes; the hooks own those.
- **Do not call `plan_task_link`** (Rust) or nav's
  `planTargetTaskUpdate`: both stamp freshness and retire schedules. Use
  the lower-level status writer (`set_task_line_status`) and the
  placement helper.

**Block-ID minting.** `mint(D)` is the first entry of bob's
`capture_block_ids::suggest_ids_with_used(description, '^', used)`.

- `description` is `note_tasks::clean_description` of D's body: the
  trailing block ID, inline fields, and the Tasks global filter are
  removed, and whitespace is collapsed.
- `used` is every block ID in D's note (staged text), plus IDs this
  gesture already minted in that note.
- When the suggester returns nothing, mint `task`, `task-2`,
  `task-3`… (first free), via `capture_block_ids::mint_block_id`.
- The ID is deterministic and readable. Capture and Ctrl+Enter mint
  byte-identical IDs; §11.7 SB vectors pin both.

**Link form.** Same rules as dependency links (§3):

1. `[[basename#^id]]` when the basename is unique;
2. otherwise `[[dir/note#^id]]`.

The one exception: a successor that lives in the day file itself still
names the note (`[[20261009#^id]]`), never the bare `[[#^id]]`. Rust
uses `task_dependencies::format::canonical_link` with the same
`NoteIndex` capture uses for `&` dependency links. That index covers
every eligible Markdown note the discovery walks — task-bearing or
prose-only, including untyped root notes, nested folders, ref notes,
terminal projects, daily notes, hidden tasks, and
completed/cancelled/archive history, excluding dot-directories,
`_templates`, `_generated`, `_conflicts`, and the other
always-excluded names — unioned with the batch's staged `.md` files.
The JS port counts the same set without reading note bodies.

### 12.5 Reporting model

The capture result contract is additive under schema version 1, so old
clients keep working. Dry-run JSON equals real-run JSON except for
`dry_run`. There is no new subcommand and no new CLI option.

Both `task_complete` and `pomodoro_close` objects carry the same three
fields:

```json
"unblocked": [{
  "note_path": "sase.md", "block_id": "relaunch-failed-agents", "line": 82,
  "text": "Re-launch all failed agents on apollo!",
  "previous_status_symbol": "?", "previous_status_name": "Blocked",
  "status_symbol": "*", "status_name": "Next",
  "inbox": false,
  "trigger": "dependency",
  "hide_removed": false,
  "unblocked_by": [{"note_path": "sase.md", "block_id": "fix-apollo",
                    "text": "Fix apollo machine!"}],
  "link": {"day_file": "2026/20261009.md", "entry_name": "FIX", "entry_line": 37,
           "entry_created": false, "next_up": false, "line": 40,
           "block_link": "[[sase#^relaunch-failed-agents]]",
           "block_id_created": true},
  "not_linked": null
}],
"still_blocked": [{"note_path": "sase_agents_repo.md", "block_id": "badges", "line": 21,
  "text": "Start adding sase--<name> badges", "status_symbol": "?",
  "reason": "scheduled", "waits_on": 0, "scheduled": "2026-10-13",
  "trigger": "dependency"}],
"unblocked_check": "checked"
```

**`unblocked[]` rows**

- The existing fields keep their meaning; `unblocked` stays on
  `task_complete` and is new on `pomodoro_close`.
- `trigger` is `"dependency"` or `"project_empty"`. Every existing
  successor row says `"dependency"`; a missing key means an older bob.
  Project pull-in rows say `"project_empty"` (§12.10).
- `hide_removed` is true when this gesture removed a whole-token
  `#hide` from the row's task. Existing successor rows send `false`.
- The row `text` of a project (`^prj`) row is the post-write
  description with `#prj` and `#hide` dropped (display only).
- Human output uses `surfaced` / `would surface` for a pull-in that
  removed `#hide` without linking or changing status (§12.6, §12.10).
- `block_id` holds the minted ID when one was minted, and `""` when D
  has none and none was minted.
- `link` is non-null exactly when this gesture linked D.
  - Line numbers are 1-based in the day text after this item.
  - `day_file` is vault-relative.
  - `entry_name` is `""` when the entry is unnamed.
- `not_linked` is one of `already_planned`, `not_planned_today`,
  `breaker`, `project_task`, `project_member`, `hidden`, `disabled`.
  - The cycler-only model adds `failed` and `cancelled`; Rust never
    emits them.

**`still_blocked[]` rows**

- `reason` is `waits_on`, `scheduled`, or `project`.
- `waits_on` counts the open prerequisites left.
- `scheduled` is set for `scheduled`, and on a `project` row when the
  project date blocks; otherwise `null`.
- `project` (the reason) is a resident whose project still has an
  open prerequisite or a future `scheduled` date after this gesture
  (§13). The optional `project` field is that note's vault-relative
  path. Pull-in still-blocked rows carry `project` (N's path) for every
  reason. `^prj` reports only its own reasons, never a `project_*`
  reason.
- `trigger` is `"dependency"` or `"project_empty"`, same rule as
  `unblocked[]`.

**`unblocked_check`**

- `"checked"`: the lookup ran, or the gate proved it unnecessary.
- `"unavailable"`: the dependents snapshot could not be built. The close
  still succeeds and nothing is linked.
- A missing key means an older `bob`.

**Elsewhere in the result**

- **`task_complete` top-level `day_file`.** Set to the absolute day file
  path whenever the gesture changed the day file (`ledger` present or a
  successor linked). Previously it was always `null`.
- **`pomodoro_close.next_pomodoro`** names the successor's continuation
  when the successor logic created it.
- **`pomodoro_blocks[].lines[].reason`.** Optional `"unblocked"` on
  added lines that are surviving successor links.
- **Net batch reporting (SL20).** After all items are planned, a
  successor whose link no longer survives live in the final staged day
  text, or whose task is no longer open, is dropped from its item's
  `unblocked[]` and loses its block-line `reason`.
- **`task_blocks`.** Successors keep role `unblocked`.

**Shared notice model** (cycler → nav `api.notice.showUnblocked(model)`).
It is the JSON row shape above, kept in snake_case on purpose so one
vocabulary serves Rust, JS, Swift, and the vectors:

```js
{
  version: 1,
  predecessors: [{ note_path, block_id, text }],
  unblocked: [/* rows exactly as JSON, plus not_linked "failed" | "cancelled" */],
  still_blocked: [/* rows exactly as JSON */],
  daily_path: "2026/20261009.md",
  daily_content: "<post-write day text, for the plan chip>" | null,
  failure: null | { count, reason },
}
```

### 12.6 Copy

**Copy rules (shared by every surface).**

- One notice per gesture, and silence when nothing was unblocked.
- Each row reads successor text first, then the destination, then the
  cause.
- Use only existing glyphs: 🔓 unblocked (Mac `lock.open.fill`), 🔒
  still blocked (Mac `lock.fill`), ✓ done.
- Use only existing `--task-status-*` colour tokens, with no new hex
  values.
- Show at most 3 rows (linked, then not linked, then still blocked),
  then `+N more`. This is a display limit, not an insertion limit.
- Never lead with a block ID.
- Truncate task text to 48 characters with `…`.

**Notice text** (`successorNoticeText(model)` in the cycler, and the Mac
notification line). This is the plain fallback and walk-toast form:

| Situation | Text |
| --- | --- |
| One linked | `🔓 Next in FIX: Re-launch all failed agents on apollo!` |
| Several linked, one entry | `🔓 3 linked → SASE: Review memory beads, Ship AGENTS.md…, +1` |
| Several entries | `🔓 3 linked · FIX, SASE` |
| Created continuation | `🔓 Next in new BOB session (next up): Re-launch…` |
| Breaker | `🔓 7 unblocked · not linked (more than 5)` |
| Recovered only | `🔓 Unblocked: Book flights (Ready)` |
| One linked pull-in | `🔓 Next in FIX: Ship the outcome! · project has no open tasks` |
| Surfaced only | `🔓 Surfaced: Ship the outcome! (Ready) · project has no open tasks` |
| Partial failure (Obsidian) | `⚠ Closed Fix apollo machine! — couldn't link 1 successor (daily note changed)` |

**Obsidian: the `Unblocked` card** (nav's notice family,
`bob-nh-notice is-unblock`).

```text
┌──────────────────────────────────────────────────────────────────┐
│ 🔓  Unblocked   1 → FIX                     ✓ Fix apollo machine! │
│  ＋ Re-launch all failed agents on apollo!               [ Next ] │
│  🔒 Start adding sase--<name> badges · waits until Oct 13         │
│ ──────────────────────────────────────────────────────────────── │
│  (plan 3/3 · 10/10)  (added ^relaunch-failed-agents)  (Alt+N releases) │
└──────────────────────────────────────────────────────────────────┘
```

- **Header.**
  - 🔓 sits in the existing glyph slot.
  - The title is `Project empty` when every row is a project pull-in
    row (`trigger: "project_empty"`), otherwise `Unblocked`.
  - The count chip reads `n → NAME` / `n → new NAME session` /
    `n linked`, plus `· next up` when it applies.
  - The right-aligned muted receipt reads `✓ <predecessor>`, or
    `✓ N tasks`.
- **Rows.**
  - **Linked rows** show `＋ text`, a lane pill (`Next`), `↗ <note>`
    when D lives in a different note than its first predecessor, and
    `inbox` when D lives in an inbox file. Project pull-in rows add a
    muted `no open tasks` chip after the lane pill.
  - **Not-linked rows** show the text, a lane pill (`Ready`/`Next`), and
    a muted reason: `already planned`, `not planned today`,
    `not linked · more than 5`, `project task`, `hidden`, `linking off`,
    `couldn't link`, or `cancelled`. Project pull-in rows add the same
    `no open tasks` chip.
  - **Still-blocked rows** are muted: `🔒 text · waits on N more` or
    `· waits until Oct 13`. A `reason: "project"` row reads
    `· blocked by project` (`· project until <date>` when `scheduled`
    is set). Nav renders the legacy `project_dependency` /
    `project_schedule` reasons from older cycler bundles the same way.
    Still-blocked pull-in rows add `· no open tasks`.
  - Clicking a row opens that task (`app.workspace.openLinkText`).
- **Footer chips.**
  - The plan meter on the post-write daily content, which turns warn/🔴
    when over.
  - `added ^id`, or `added N block IDs`.
  - `Alt+N releases` (muted), shown when anything was linked.
- **Breaker variant.** The header reads `7 · not linked`, followed by one
  reason line: `More than 5 at once — link the ones you want with
  Ctrl+Shift+Enter`.
- **Duration.** 6 s plus 1 s per extra visible row, capped at 10 s.
- **No nav.** Without nav (or with `api.notice` missing), show a plain
  `new Notice(successorNoticeText(model))`.
- **Walk landings.** The gesture keeps its **single** walk toast. The
  notice text is composed into `outcome.notice` as the line after
  `✓ Done · <task>`, and no card is shown. Inserted rows never steal
  focus or cause a second advance.

**Bob Mac Capture: preview first, notification second.** Preview is the
primary surface because it explains before anything is written. Mac
needs no change for project pull-in (`decisions:mac-capture-is-a-thin-client`;
the JSON is additive). Pull-in rows render as linked successors.

- **`!` card unblocked rows.**
  - **Linked.** `lock.open.fill` in the Next tint, monospaced
    `[?] → [*]  Re-launch…`, and a trailing destination capsule
    (`→ FIX` / `→ new BOB · next up`). A caption reads `added ^id` when
    an ID was minted. An `inbox` capsule appears when it applies.
  - **Not linked.** `lock.open.fill` in secondary colour,
    `[?] → [ ]  text`, and a caption with the reason
    (`Ready · not planned today`).
  - **Still blocked.** `lock.fill` in tertiary colour, `text`, and a
    caption reading `stays Blocked · waits on 1 more` /
    `· until Oct 13`.
- **`=x` close card.** Gains an **Unblocked** section under its task
  rows, with the same row views.
- **Block diff.** An added line with `reason == "unblocked"` gets a small
  trailing `lock.open.fill` badge with `.help("Unblocked successor")`,
  so the successor visibly sits under its struck predecessor.
- **Notification.** One extra body line, taken from the notice-text table
  above. There is never a second notification. With only recovered rows,
  keep today's `· unblocked A, B`.
- **Open Note(s)** includes the daily note whenever anything was linked.

**Human output** follows the existing word-led rows (`dry run`:
`would link`). It prints after the `ledger` line for `!`, and after the
task rows and before `next:` for closes:

```text
✓ completed [*] → [x] Fix apollo machine!  sase.md ^fix-apollo
  ledger  Task Link struck in FIX · 2026/20261009.md
  linked [?] → [*] Re-launch all failed agents on apollo!  sase.md ^relaunch-failed-agents → FIX · added ^relaunch-failed-agents
  unblocked [?] → [ ] Book flights  travel.md ^book-flights · not planned today
  linked [ ] → [*] Ship the outcome!  proj.md ^prj → FIX · project has no open tasks
  surfaced Ship the outcome!  proj.md ^prj · project has no open tasks
  still blocked  Start adding sase--<name> badges  sase_agents_repo.md ^badges · until 2026-10-13
  still blocked  Ship the outcome!  proj.md ^prj · project has no open tasks · waits on 1 more
plan 3/3 themes · 10/10 links
```

- **Destinations:**
  - `→ FIX`
  - `→ FIX (next up)`
  - `→ new BOB session (next up)`
  - `→ new session` for an unnamed created entry
  - `→ line 37` for an unnamed existing entry
- **Reason suffixes:**
  - `· already planned`
  - `· not planned today`
  - `· project task`
  - `· hidden`
  - `· not linked, more than 5`
  - `· linking off`
- **Project pull-in.** A pull-in that linked R uses `linked` / `would
  link`. A pull-in with no link and no status change uses `surfaced` /
  `would surface`. Every project row puts `· project has no open tasks`
  before any reason suffix.
- **Still-blocked suffixes (Rust human):** `· waits on N more`,
  `· until YYYY-MM-DD`, `· project <path>`. The nav card keeps
  `· blocked by project` and `· project until <Mon D>`.
- A row with no block ID omits ` ^`, as today.

### 12.7 Performance

| Layer | What | Effect |
| --- | --- | --- |
| 0. `[id::]` gate | Before any lookup, check the **staged** post-close lines of C for an `[id::]` field. | Zero extra I/O for about 96% of closes, in Rust and JS. It also speeds up today's Ctrl+Enter recovery. |
| 1. Lazy context | `DependencyContext` builds `discover` only for items that need the dependency pool. `!` resolves notes from a walk-only index. | `hello` / `=x` previews go from about 270 ms to about 40 ms on apollo. |
| 2. Prefiltered parallel snapshot | One batch-scoped dependents snapshot. Byte reads run in parallel, and a prefilter keeps only notes containing `dependsOn` or `id::`. Staged files always overlay disk. Shared by every `!` and close item in the batch, never cloned per item. | About 30 ms per batch, paid only when the gate passes. |
| 3. Obsidian Tasks cache | When Tasks reports Warm, build a reverse index `id → dependents` from `getTasks()`. Open editor buffers override the cache. Read only the candidate notes and today's daily note. A cold cache falls back to the full scan. | The notice lands about 50 ms after the keypress. |
| No daemon, no persistent cache | A validated cache needs a stat walk (32–55 ms) that costs as much as layer 2. | Respects the thin-client rule: no Swift-side dependency or placement logic. |

**Targets.** Measured on apollo with a release build, against a temporary
**copy** of `~/bob` (never the live vault):

- `hello` and `=x` dry runs take ≤ 40 ms;
- a `!` or `=x!` that closes a real prerequisite takes ≤ 70 ms;
- `capture-parse` does no new work;
- no extra spawn is needed to report results.

### 12.8 Kill switch

`plan.link_unblocked: true` lives in `~/.config/bob/config.yml` and is
read by Rust and the cycler. Setting it to `false` turns off **linking
and minting only**:

- close-time recovery still runs and still uses the derived rank;
- each unblocked row reports `not_linked: "disabled"`;
- project pull-ins obey the same kill switch: no link, status
  unchanged, `not_linked: "disabled"`; un-hiding still happens
  (§12.10).

Recovery is the honest half of the feature and costs nothing extra once
the dependents are found. An invalid value falls back to `true` and
surfaces through the existing invalid-plan-config warning paths
(`docs/plan.md` "Invalid values").

### 12.9 Undo

- **One successor.** Alt+N on its link releases it, as the inverse of
  what was written. The card's chip teaches this.
- **The whole gesture, in Obsidian.** Reopening the predecessor the same
  day takes back its successor links if they are untouched. This uses an
  in-memory receipt; nothing is stored in Markdown. For a project
  pull-in, the receipt also stores R's full pre-edit line and restores
  that line byte-for-byte (status and `#hide`) when R's current line is
  still the line the pass wrote (§12.10).
- **Not undo.** Deleting the bullet or pressing `u` in the editor leaves
  the sticky Next. That is the general sticky-lane cost.

### 12.10 Project pull-in

When a close gesture empties a project, do immediately and visibly what
`bob projects sync` would do within 15 minutes, and also link `^prj` as
Next. "Empties" uses sync's own predicate, so sync never re-hides a
pulled-in `^prj`. Implementations copy the PP table (§12.11) into their
tests, citing the vector id in a comment. If an implementation disagrees
with a vector, fix this section first in the same change.

A pulled-in `^prj` is never Blocked when linked (Rule steps 1–3), so
§13.5 consumption and "never link a Blocked task" hold. Dependency
promotion never raises a `^prj` target; a direct Task Link raises `^prj`
to Next like any other task, including this close-gesture pull-in.

**Terms**

```text
C           = tasks this gesture moved open → Done (§12.2)
N           = a project note: outside done/, not a canonical daily note,
              frontmatter type project, non-terminal status, exactly one
              open ^prj task R (bob projects sync's recognition), AND
              sync can plan N: its scan reports no issue for the file
              (e.g. exactly one ^prj-anchored line, at most one valid
              inline [scheduled::] on ^prj). The effective status is
              non-terminal as sync computes it.
REMAINING(N)= sync's effective-unhidden count on the staged text: open
              #task lines other than R, outside fenced code, whose mark
              is not x / X / -, carrying no whole-token #hide
              (= sync's open_unhidden_count: [?], custom marks, nested,
              #ref and blockquoted lines count) …
              … except when a project schedule applies (see SCHED). Then
              a hidden ordinary open task with fewer than two scheduled
              fields also counts, because sync strips its #hide.
SCHED(N)    = sync's project schedule: a leftover frontmatter
              `scheduled` on an open ^prj, else R's inline
              [scheduled::]. Rule step 2 compares SCHED(N), not only
              R's inline field, with today.
SUBOPEN(N)  = R's machine-owned "🧩 **Sub-projects:**" marker line
              lists a child that sync's marker reading treats as open,
              i.e. anything but ~~[[child]]~~ ✅ (Done) or
              ~~[[child]]~~ ❌ (Canceled). A link struck without ✅/❌
              is still open.
EMPTY(N)    = REMAINING(N) = ∅ and not SUBOPEN(N), on the staged
              post-gesture text
```

SUBOPEN reads the marker line instead of child notes' `parent:`
frontmatter, which would need a vault-wide walk. The marker is
sync-maintained (rewritten at most every 15 minutes), so it can be
stale. Both stale outcomes are benign: a missed pull-in (sync surfaces
R on its next run instead), or an extra one (sync re-hides R, and R
stays Next until Alt+N or a close).

**Trigger**

A pull-in candidate is the open `^prj` R of a project N such that
EMPTY(N), R ∉ C, and one of:

- (a) C contains a task in N other than R: the gesture closed N's last
  task.
- (b) R is a dependent candidate of C (its `[dependsOn::]` names a task
  in C). This gesture removed R's last blocker.

- (c) C contains the `^prj` of a project K with `parent(K)` = N,
  R ∉ C, and EMPTY(N) holds with N's marker entry for K read as
  closed. `preds(R)` = {K's `^prj`}, anchored by its slot, the
  closing entry, or inherit (§12.3). Reuses `trigger:
  "project_empty"`, its copy, and its rows; `unblocked_by` names K's
  `^prj`. Cost: one parent resolution plus one read of N, and only
  when C contains a closed `^prj` whose note has `parent:`. Every
  other close is unchanged.

`preds(R)` is the set of tasks in C that triggered R: N's closed tasks,
R's closed prerequisites, or — under trigger (c) — K's `^prj`.

**Rule** (both engines)

```text
FOR EACH candidate R (one row per R; a pull-in replaces R's SL11
project_task row):
  1. R has open prerequisites after the gesture
                                  → still_blocked{reason: waits_on}
  2. SCHED(N) > today             → still_blocked{reason: scheduled}
  3. R carries `#prj_is_blocked`
                                  → still_blocked{reason: project,
                                     project: parent(N)}
     (the tag means N inherits a reason from an ancestor, so a
     pull-in still never links a Blocked ^prj. this applies to
     every trigger. steps 1–3 leave R's line byte-identical,
     #hide included)
  4. From here on, remove every whole-token #hide from R.
  5. R had a live link before the gesture
                                  → not_linked: already_planned;
                                    ' ' / '?' → '*'
  6. no task in preds(R) has an anchor
                                  → not_linked: not_planned_today;
                                    recover rank
  7. cancelled close (cycler only)
                                  → not_linked: cancelled; recover rank
  8. plan.link_unblocked is false → not_linked: disabled; recover rank
  9. otherwise                    → PULL-IN: link R at the earliest
                                    anchor among preds(R) (§12.3 slot /
                                    closing / inherit); ' ' / '?' → '*';
                                    '*' and '/' unchanged
recover rank : '?' → '*' if R has a live link in the post-gesture day
               text, else ' '; other statuses unchanged. A Ready
               landing (`' '`) writes `[unblocked::]` when
               freshness.md §5's WRITE rule applies; a Next landing
               skips it (UB4). The WRITE rule is evaluated on R's
               line AFTER step 4 removed #hide, so the hidden skip
               never applies to a pulled-in R. When R was `[?]` and
               leaves Blocked through steps 5–9, §12.2 PROJECT
               RECOVERY runs for N exactly as for a `project_task`
               recover (PDC2/PDC3).
BREAKER      : pull-ins never count toward the 5-successor breaker and
               are never broken
ORDER        : pull-ins sort with successors (§12.2 ORDER: anchor, note
               path, line)
ROWS         : unblocked[] gets a row when the gesture changed R's
               status, removed #hide, or linked R. still_blocked[]
               always gets a row for steps 1–3. An already-Next,
               already-linked, visible R produces nothing.
               A surfaced row removed #hide without linking or changing
               status: `previous_status_symbol` == `status_symbol`,
               `hide_removed: true`, `link: null`, and its `not_linked`
               reason.
```

Status follows the successor rule: ` ` and `?` become `*`; `*` and `/`
are unchanged. A Pending `^prj` is never demoted
(`decisions:task-lanes-are-sticky`).

**Gate.** The `[id::]` gate still guards the dependents lookup only.

- Detecting (a) reads only the staged post-gesture text of C's own
  notes. Both engines already hold that text.
- The vault-wide dependents snapshot is built only when needed:
  - some task in C carries `[id::]` (as today); or
  - a candidate R carries `[dependsOn::]`. Steps 1 and 3 need
    vault-wide openness in that case; or
  - a candidate R has status `?` and some open resident of N carries a
    non-empty `[dependsOn::]`. The §12.2 PROJECT RECOVERY cascade checks
    residents' own prerequisites vault-wide in that case.
- If the snapshot is unavailable, set `unblocked_check: "unavailable"`
  and write no pull-in.
- Closes outside project notes cost nothing extra. The cycler's
  zero-read wiring test must keep passing.

**Writes**

- **R's line.**
  - The status changes per the Rule.
  - Every whole-token `#hide` is removed. `#hidden` and `#hideaway`
    stay.
  - Whitespace is byte-identical to sync: Rust calls
    `projects::edits::normalize_task_hide_tag(line, false, true)`. The
    cycler mirrors it. PP22 pins the bytes.
  - Nothing else changes except a Ready recover-rank landing, which
    writes `[unblocked::]` when freshness.md §5's WRITE rule applies.
    No freshness stamp, no schedule, no `[id::]` / `[dependsOn::]`.
    `^prj` already has its block ID, so nothing is minted.
- **Day file.** One `<indent>- [[N#^prj]]` bullet, placed per §12.3.
  The link form follows §12.4, so it is never `[[#^prj]]`.

**Reporting** (additive, capture schema v1)

- Every `unblocked[]` and `still_blocked[]` row carries
  `trigger: "dependency" | "project_empty"`. Every existing row says
  `"dependency"`; a missing key means an older bob. Pull-in rows say
  `"project_empty"`.
- `unblocked[]` rows carry `hide_removed` (bool).
- The row text of a project row is the post-write description with
  `#prj`, `#hide`, and `#prj_is_blocked` dropped (display only,
  token-bounded).
- `unblocked_by` lists `preds(R)`.
- **Human output (Rust).**
  - A pull-in that linked R uses `linked` / `would link`.
  - A pull-in with no link and no status change uses `surfaced` /
    `would surface`.
  - Every project row puts `· project has no open tasks` before any
    reason suffix.
- **Notice text** (cycler `successorNoticeText`, nav fallback, Mac
  notification line).
  - One linked pull-in:
    `🔓 Next in FIX: Ship the outcome! · project has no open tasks`.
  - Surfaced only:
    `🔓 Surfaced: Ship the outcome! (Ready) · project has no open tasks`.
  - Mixed notices keep §12.6's count forms, and a pull-in counts as
    linked.
  - Only existing glyphs are used.
- **Nav `Unblocked` card.**
  - Project rows get a muted `no open tasks` chip after the lane pill.
  - The title is `Project empty` when every row is a project row,
    otherwise `Unblocked`.
  - Still-blocked project rows add `· no open tasks`.
- **Bob Mac Capture** needs no change. Pull-in rows render as linked
  successors.

**Undo (Obsidian)**

The successor receipt also stores R's full pre-edit line. A same-day
Ctrl+Enter reopen of a predecessor that owns the row does two things,
as long as R's current line is still the line the pass wrote:

- takes back R's link, as §12.9 does today;
- restores R's whole line (status and `#hide`).

Rust has no reopen take-back, as today.

**Interplay**

- **`bob projects sync`.** After a pull-in it changes nothing for N
  (same predicate). When Bryan then adds a task to N, sync re-adds
  `#hide`. R stays Next with its link under sticky lanes, and Alt+N
  releases it, or Bryan closes R.
- **`bob task reconcile`.** A linked `[*]` R is stable. If N later
  becomes Blocked, §13.5 consumption removes R's link.
- **Freshness / walk.** Untouched except the recover-rank
  `[unblocked::]` (§12.4). A visible R walks in the PROJECTS tier as
  today.
- **Multi-item drafts.** SL20's net pass drops any pull-in row, linked
  or not, whose R is no longer open in the final staged text.

### 12.11 PP — project pull-in vectors

Outcomes of the shared project pull-in rule (§12.10). Rust tests run
them through capture (`!` and close); cycler tests run them through the
pure helpers. Copy the rows you implement into tests, citing this
section in a comment.

| # | Situation | Outcome |
| --- | --- | --- |
| PP1 | N has one open task A, linked in FIX; R is ` ` with `#hide`; close A | R is linked right after A's bullet; ` `→`*`; `#hide` removed; `trigger: project_empty`; `hide_removed: true`; `unblocked_by: [A]` |
| PP2 | A and B are open; close A | nothing |
| PP3 | B is `[?]` | nothing (Blocked tasks are remaining work) |
| PP4 | B carries `#hide` | pull-in |
| PP5 | B is fenced | pull-in. B is blockquoted or custom `[>]` → nothing |
| PP6 | Sub-projects line with an unstruck child, or `~~[[child]]~~` struck without ✅/❌ | nothing (both read as open). Only ✅/❌ children → pull-in |
| PP7 | future project `scheduled` on `^prj` | `still_blocked{reason: scheduled}` with the date; `trigger: project_empty`; R byte-identical |
| PP8 | R has an open prerequisite on its Depends-On line | `still_blocked`; R byte-identical |
| PP9 | R already linked today | no new link; ` `→`*`; `#hide` removed; `already_planned` |
| PP10 | A not planned today | no link; status unchanged; `#hide` removed; `not_planned_today`; row reported (verb `surfaced`) |
| PP11 | `plan.link_unblocked: false` | as PP10 with `disabled` |
| PP12 | R is `[/]` | linked; stays `[/]` |
| PP13 | R is `[*]`, visible and already linked | no write, no row |
| PP14 | A has no `[id::]` | the gate does not suppress the pull-in, and the dependents snapshot is not built |
| PP15 | a dependency successor D and R in one gesture | both linked after A's bullet in ORDER. With six dependents plus R → six `breaker` rows, and R still linked |
| PP16 | `=x` completes embedded A | R joins the closing target (continuation, same-name entry, or created placeholder) |
| PP17 | R depends on A through a legacy internal Depends-On line | one row, and the pull-in wins over `project_task` |
| PP18 | cycler cancel of A | no link; `#hide` removed; `cancelled` |
| PP19 | same-day reopen of A (Obsidian) | R's link taken back and R's line restored byte-for-byte |
| PP20 | terminal project, two `^prj`, N under `done/`, a canonical daily note, or N with a sync scan issue (e.g. two inline dates on `^prj`) | nothing |
| PP21 | draft `!N:a !N:b` | only the second item reports R. Draft `!N:b !N:prj` → the net pass drops R's row |
| PP22 | `#hide` bytes via `normalize_task_hide_tag(line, false, true)` | `- [ ] #task #prj Ship it! #hide ^prj` → `- [ ] #task #prj Ship it! ^prj` (pull-in then writes `[*]`); `- [ ] #task #prj Ship it! #hide [priority:: high] ^prj` → `- [ ] #task #prj Ship it! [priority:: high] ^prj`; `- [ ] #task #prj Ship it! #hidden #hide ^prj` → `- [ ] #task #prj Ship it! #hidden ^prj` (`#hidden` untouched) |
| PP23 | after a pull-in, `bob projects sync --dry-run` | reports no change for N |
| PP24 | closing P in another note when R depended only on P and N is empty | R pulled in after P's bullet. If N is not empty → `project_task` (unchanged) |
| PP25 | R is `[?] … #hide` and depends only on P in another note. N's only other open task is `[?] #prj_is_blocked #hide`. Close P (planned today) | R is pulled in after P's bullet: `[?]`→`[*]`, R's `#hide` removed. The resident loses `#prj_is_blocked` and recovers to `[ ]` as `project_member`. It gets no `[unblocked::]` (it is still hidden, UB6) |
| PP26 | R carries `[scheduled:: <today or past>]`; N's only other open task B carries `#hide`. Close A | nothing (B is remaining work under the dated recount). With no project date → pull-in |
| PP27 | R `[?]` with `[fresh::]`, not planned today, last blocker closed | `' '`, `#hide` removed, `[unblocked:: today]` written, `not_planned_today`. Linked instead → `[*]` and no `[unblocked::]` (UB4) |
| PP28 | leftover frontmatter future `scheduled` on N with an open `^prj` | `still_blocked{reason: scheduled}`, R byte-identical |
| PP29 | K (`parent: "[[N]]"`) is the only open sub-project on N's marker line, and N has no other remaining work. K's `^prj` is linked in FIX; close it via its link | N's R is linked right after K's bullet: ` `→`*`, `#hide` removed, `trigger: project_empty`, `unblocked_by: [K ^prj]` |
| PP30 | N's marker lists another open child, or N has a remaining task | nothing |
| PP31 | K has no `parent:`, or it is unresolvable, ambiguous, or not a project | nothing |
| PP32 | R carries `#prj_is_blocked` (N inherits), under any trigger | `still_blocked{reason: project, project: parent(N)}`, `trigger: project_empty`; R byte-identical |
| PP33 | K's `^prj` not planned today; cycler cancel (Alt+[) of K's `^prj` | surfaced (`not_planned_today`); cancel → `cancelled`, surfaced only |
| PP34 | after a PP29 pull-in, `bob projects sync --dry-run` | changes only N's marker entry for K (→ `~~[[K]]~~ ✅`), never N's `#hide` |
| PP35 | draft `!K:prj !N:prj` | the net pass drops R's row |
| PP36 | row text of a project row whose R carries `#prj_is_blocked` | drops `#prj_is_blocked` with `#prj` and `#hide` (token-bounded) |

## 13. Project dependencies

A project's (`^prj`) prerequisites live on its ordinary Depends-On
first-child line, and its date as an inline `[scheduled::]` on `^prj`.
While `^prj` has its own reason to be Blocked — an open projected
prerequisite, or a future inline `scheduled` — every open task in its
note is Blocked `[?]` and tagged `#prj_is_blocked`. No Blocked task
keeps a Task Link under today's open Pomodoros: the link is consumed
(prerequisites promoted, link removed, emptied Pomodoro deleted).
Ordinary tasks, including ordinary tasks inside project notes, keep
the Depends-On line and R1–R10 unchanged. `^prj` uses those same
R-rules: no R0, no D3, and no R2 exception. Blocking also cascades
to sub-projects: a blocked project's descendant project notes
inherit its `^prj` reasons through `parent:` (§13.1, §13.3).

Implementations copy the PD tables in this section into their tests,
citing the vector id in a comment. If an implementation disagrees with
a vector, fix this section first in the same change. `[unblocked::]`
vectors live in [freshness.md §10b](freshness.md#10b-ub--unblocked-vectors).

### 13.1 Vocabulary

- **Project dependency:** a dep link on a Project Task's (`^prj`)
  Depends-On line. Project reasons are `^prj`'s own reasons.
- **Project (`project(T)`):** the note T lives in, when that note is
  `type: project`, has a non-terminal status, has exactly one open
  `^prj`, and is not a canonical daily note (`YYYY/YYYYMMDD.md`).
- **Resident task:** every recognised open `#task` line physically in
  the project note except `^prj`, including nested and `#ref` tasks.
  Fenced, blockquoted, and custom-status lines are excluded. Residence
  is physical; the cascade through `parent:` below is the one
  exception.
- **Parent (`parent(K)`):** the note named by K's frontmatter `parent:`,
  read as one scalar wikilink (alias and heading dropped, any path
  kept) and resolved like the note index (exact vault-relative path,
  else the unique case-insensitive basename among scanned notes).
  Unresolved, ambiguous, list-form, or K itself → no parent.
- **Ancestors (`ancestors(K)`):** `parent(K)`, `parent(parent(K))`, …
  while each is `project(·)`, stopping at the first note that is not,
  at a repeat (cycle), or at depth 16.
- **Descendants (`descendants(N)`):** every project K with
  N ∈ `ancestors(K)`.
- **Inherited reasons (`inherited(K)`):** for each A in `ancestors(K)`,
  nearest first, with `own_reasons(^prj` of A`) ≠ ∅`: the
  `project_dependency` / `project_schedule` reasons that name A.
- **Project reasons (`project_reasons` / `blocked(P)`):** `own_reasons`
  of P's `^prj` union `inherited(P)`. Internal targets are never
  projected, so they never contribute.
- **Own reasons (`own_reasons(T)`):** open prerequisites projected from
  T's Depends-On line (`local_dependency`) union `{ T.scheduled > today }`
  (`local_schedule`).
- **Consumption:** promoting a Blocked task's open prerequisites along
  `edges(T)`, then removing its live Task Links under today's open
  Pomodoros, then running the empty-Pomodoro rule.
- **`#prj_is_blocked`:** machine-owned tag on every open resident of a
  blocked project, and on the `^prj` of a project that inherits a
  reason from an ancestor (§13.7). A `^prj` carries the tag ⇔
  `inherited(K) ≠ ∅`.

### 13.2 Storage

The `^prj` line is an ordinary task:

```markdown
---
type: "[[project]]"
status: wip
---

- [?] #task #prj Apply to DataBricks NYC role(s)! [fresh:: 2026-10-12] [scheduled:: 2026-10-20] #hide [dependsOn:: job__update-linkedin] ^prj
  - ⛓️ **DEPENDS ON:** [[job#^update-linkedin]]
  - 🧩 **Sub-projects:** …

## Tasks

### Blocked

- [?] #task #ref [[…|…]] #prj_is_blocked ^ref-databricks-nyc-cv-and-role-pitches
```

- **Prerequisites.** `^prj`'s prerequisites sit on its ordinary
  Depends-On first-child line, exactly as on any task. They are
  reconciled by R1–R10. That line is first, or second after a
  `❌ **CANCEL LOG**` child. The machine-owned `🧩 **Sub-projects:**`
  line follows it.
- **Date.** The project date is `^prj`'s inline `[scheduled:: YYYY-MM-DD]`,
  placed like any task's schedule. Neither field lives in frontmatter.
- **Hoist.** `bob projects sync` hoists any leftover frontmatter
  `scheduled` onto `^prj` and deletes the key:
  - frontmatter wins when both exist;
  - terminal projects are left alone;
  - an invalid value stays in place with a warning.

  The hoist is permanent, which also makes it a shim for old nav
  builds and Properties edits. Report it as
  `hoisted_project_schedules`.
- **Internal targets.** A `^prj` link to a task in its own note is a
  guaranteed deadlock: pickers refuse it; reconcile keeps the link on
  the line but does not project it into `[dependsOn::]`, and warns
  `project_internal_dependency`; JS project tests already ignore the
  project's own ids.

### 13.3 Effective blocking

```text
own_reasons(T)  = open prerequisites projected from T's Depends-On line   (local_dependency)
                ∪ { T.scheduled > today }                                 (local_schedule)
project(T)      = the note T lives in when: type project, non-terminal status, exactly
                  one open ^prj, not a canonical daily note
blocked(P)      = own_reasons(^prj of P) ≠ ∅ OR inherited(P) ≠ ∅
                  (internal targets never projected)
^prj of K is Blocked ⇔ own_reasons ≠ ∅ OR inherited(K) ≠ ∅
                  (it reports inherited reasons naming A, never itself)
resident T of K is Blocked ⇔ own_reasons(T) ≠ ∅ OR blocked(K)
                  → project_dependency / project_schedule
edges(T)        = T's own dep links ∪ (T is a resident ? ^prj's projected links : ∅)
                  ∪ (T is ^prj(K) or a resident of K ? projected links of every
                     ^prj(A), A ∈ ancestors(K) : ∅)
internal(^prj N)= targets in N's own note OR in any descendant of N: kept on the
                  line, never projected into [dependsOn::], warn
                  project_internal_dependency, refused by pickers and capture &
```

- **Residents** are every recognised open `#task` line physically in
  the note except `^prj`, nested and `#ref` tasks included. Fenced,
  blockquoted, and custom-status lines are excluded.
- **The cascade.** A sub-project inherits every ancestor's `^prj`
  reasons through `parent:`, recursively: the inheriting `^prj` and
  every open resident are `[?]` and tagged. Each cycle member inherits
  from the others' own reasons, never from itself; reconcile warns
  `project_parent_cycle` once per cycle.
- **`^prj` reports own plus inherited reasons.** It never gets a
  `project_*` reason that points at itself.
- **Derive from reasons, never from the `[?]` cache.** A hand-set `[?]`
  on `^prj` blocks nobody.
  - JS immediate paths may use `^prj`'s `[?]` as the "will the project
    still be blocked" shortcut.
  - JS paths use the resident's own `#prj_is_blocked` tag as the "this
    resident is project-blocked now" signal.
  - Engines that do not walk the tree read a tagged `^prj` as an extra
    project reason naming `parent(K)` (the tag proxy).
- **Recovery** happens only when every reason, own and inherited, is
  gone. The task returns to its existing derived rank. No previous
  status is stored. Lanes do not survive Blocked, now project-wide.
- **Reasons are reported, not stored.** The kinds are
  `local_dependency`, `local_schedule`, `project_dependency`, and
  `project_schedule`, with project path, open ids, and date. A
  `project_*` path may now name an ancestor that owns the reason. They
  appear in reconcile JSON, capture JSON, and plugin notices.
- **ADJ-11.** Dependency promotion never raises a `^prj` target into a
  lane, but the walk continues through it to that project's
  prerequisites. A direct Task Link raises `^prj` to Next like any
  other task.
  Residents of a depended-on project are not promoted.
- A terminal project (`status: done` or `canceled`) is not
  `project(T)`, so the chain stops at the first non-project note: its
  descendants inherit nothing from above it. A note with more than one
  open `^prj` is not a project under this rule. A canonical daily note
  typed `project` is ignored.

### 13.4 Promotion

`edges(T)` is the promotion, recovery, and R7 graph.

- Dependency promotion never raises a `^prj` target into a lane.
  ADJ-11 governs that rule. A direct Task Link raises a `^prj` to
  Next like any other task, including the §12.10 close-gesture pull-in.
- The walk continues through that `^prj` to the depended-on project's
  own Depends-On links.
- Ancestor edges: a `^prj` and its residents also link to every
  ancestor `^prj`'s projected targets. This feeds promotion,
  consumption seeds, and R7 cycle detection.
- Residents of a depended-on project are not promoted.
- Adding a project dependency from the keymap **transfers
  commitment:** each open prerequisite rises to at least the strongest
  lane among `^prj` and its residents. That transfer never lands on a
  `^prj` target.

### 13.5 Consumption

**Scope.** Every effectively Blocked task, not only project residents.
Only live (unstruck) Task Links under **today's open** Pomodoros.
Closed entries, `🍅` history, other days, and dependency links are
never touched.

**Removal.** A link-only bullet is removed with its subtree; otherwise
only the link token is removed. Then the existing empty-Pomodoro rule
runs; it may delete a running timed entry (accepted; matches cancel).

**Promotion seed.** Before removal, the link seeds promotion exactly
as a surviving link would, along `edges(T)`, so the task's open
prerequisites (own plus inherited) rise to at least Next.

**Final-state test.** A link whose target's last reason clears in the
same run is kept.

**Gesture rule.** Any gesture that would create or keep a live link to
a task that is Blocked after the gesture follows one rule:

- only the task's **own** future `scheduled` blocks it → pull forward
  as today (retire the schedule, link it);
- a dependency or project-dependency reason → capture may drop the
  dependent's own open prerequisites under §13.5.1; otherwise do not
  link, promote the open prerequisites (own plus inherited), and report
  it;
- a `project_schedule` reason without a dependency reason → do not
  link; report the project date;
- **refinement: a `^prj` target whose only reason is its own date** →
  refuse, with "move the project date from the `^prj` Schedule
  action". This is never a pull-forward, which would silently unblock
  the project and strand residents' copies.

Capture never creates a linked `[?]` task: it creates it unlinked and
reports `not_linked` with the reasons. Links under a Pomodoro the same
gesture closes are history, not live links.

### 13.5.1 Capture drops its own prerequisites before linking

Capture link gestures may link an existing task that is Blocked only by
its own open prerequisites by removing those prerequisites from its
Depends-On line and `[dependsOn::]` field, then running the ordinary
link gesture. The task is unblocked before its Task Link is added. This
applies to solo `@route:id` / `^route:id`, `@route+id!`, Ensure Next,
and link-then-close. A single own future `[scheduled::]` may accompany
the dependencies and is retired by the existing pull-forward path.

The drop is capture-only. It requires a local dependency reason, no
project or inherited project reason, a target other than a recognised
project's `^prj`, no unsupported multiple future schedule fields, and
exactly one accepted managed Depends-On child. If any gate fails,
today's consumption or project-task refusal remains in force. When a
local dependency reason prevents a drop, `link_guard.drop_refused`
reports `project_blocked`, `project_task`, `schedule`, or
`dependency_line`, and the human detail says why the prerequisites
were kept.

The writer removes every link to each open own prerequisite, including
duplicate links. Kept links remain verbatim and in order; prerequisite
tasks are never edited. Their corresponding IDs are removed from the
dependent's `[dependsOn::]` field, while unrelated and historical IDs
remain. An empty Depends-On line and field are removed. The dependent is
stamped fresh, then the ordinary capture link write runs in the same
staged batch; a later failure leaves the drop unapplied. No
`[unblocked::]` is written because a successful link puts the task in
Next. Dropped links are reported in `dropped_dependencies` and human
output; the task itself does not keep a history edge. Plugins retain
the PDG1 consumption behavior.

**Pipeline.** Compute the final Blocked set (with project reasons),
then plan the ledger in this order: dedupe, remove canceled links,
**consume blocked links**, retire completed links and repair markers,
remove empty Pomodoros. A second rewrite pass after transitions is an
accepted alternative. Either way: dry-run output equals the live
write, and a second run changes nothing (except the daily-note
exception below).

**Known exception.** A prerequisite in a canonical daily note promoted
only by consumption follows the sticky-lanes R9 carve-out and may
clear on the next run.

### 13.6 Guards

- Pickers refuse `^prj` targets inside the same project note, and
  targets inside any descendant of the project ("inside a sub-project
  of this project"). A `^prj` that depends on its own sub-project's
  task would block that task forever — the same guaranteed deadlock
  §13.6 already prevents for same-note targets. A hand-authored
  internal target stays on the line, is not projected onto
  `[dependsOn::]`, and warns `project_internal_dependency`, naming the
  sub-project for descendant targets.
- Cycle detection (reconcile R7, capture, nav stage) includes the
  implicit resident → `^prj` edges. Longer cycles keep the R7
  behavior (keep and warn).

### 13.7 `#prj_is_blocked`

**Rule.** An open resident of a blocked project carries exactly one
`#prj_is_blocked`, and a `^prj` carries it ⇔ it inherits a reason
from an ancestor. No other open task carries it.

- Closed lines are never edited; a tag left on a closed line is
  accurate history.
- Every open resident is tagged, including residents that also have
  their own reasons (`tag_scope = every_resident`).
- An inheriting `^prj` is tagged exactly once; placement is as PDT9.

**It is machine-owned derived state,** like `[?]` and `#hide`.

- **Reconcile is the authority.** It adds the tag, and strips it from
  open lines that no longer qualify (moved out, project unblocked,
  project terminal, hand-copied, or a `^prj` whose ancestors cleared).
- **The Task Card subtree pass is the immediate cascade writer.**
  After a Depends on edit — or a Schedule edit — to N's `^prj`, it
  blocks and tags N's descendants top-down in the same gesture, and
  recovers them when N's last reason clears (§13.10).
- **Same-note writers keep their own note current in the same
  write** wherever they flip a resident: capture (`&` on `^prj`, new
  tasks into a blocked project); nav (the Depends on stage on `^prj`,
  Ctrl+Shift+M, conversion); and cycler/capture recovery of residents.
- **Every other cascade write defers to reconcile.** Capture `&` on
  `^prj`, close recovery, Alt+`]`/Alt+`[` on a `[?]` `^prj`,
  Ctrl+Shift+M, conversion, and new child projects respect the tag
  proxy but write no descendant note: a close that unblocks a project
  leaves its descendants tagged until the next reconcile (≤ 15 minutes
  on the MacBook cron), which re-derives them.
- A missed immediate write is repaired within one reconcile interval.

**Placement.** Use `projects/edits.rs` `task_metadata_insertion_offset`,
which puts it after the fields and before `^id`. Both Rust parsers and
the freshness placement rule already treat trailing tags as part of
the Tasks suffix. In reconcile, tag edits are line rewrites and count
as structural, so the quiet period applies.

**Matching hazards.**

- Tasks and Dataview `tags include #prj` is a substring match.
  Document it; no vault query depends on it today.
- Every engine's own `#prj` / `#prj_is_blocked` matching must be
  token-bounded. Each phase audits its engine.
- Add the tag to `projects/tags.rs`' description strip list.

### 13.8 Project schedule side effects

- **Propagation stays residence-only.** Sync and nav still copy the
  date onto residents, raise-only, but now read it from `^prj` inline.
  Project dates are never propagated to descendant notes.
- **Move-earlier fix (nav only):** when a gesture changes `^prj`'s
  date from P_old to P_new, also replace residents' copies that are
  exactly P_old. Ctrl+D already removes copies by exact value.
  - Rust sync cannot know P_old. A hand edit that moves the date
    earlier keeps the old copies; that is a known limit.
- **`#hide` forcing** for a future date stays in sync.
- **Stamps:** `^prj` follows the normal stamp rule. The
  "project-frontmatter edits never stamp" carve-out is gone.
- **Due or deleted dates follow `^prj` status.** Residents lose
  `#prj_is_blocked` when `^prj` leaves Blocked. A closed Depends-On
  history line does not keep the tag. Residents keep it while `^prj`
  stays `[?]` (an open prerequisite or a still-future date). Reconcile
  repairs the tag once the last remaining reason clears.

### 13.9 Outputs and warning kinds

Additive reconcile output:

- `consumed_blocked_references`: `{path, line, pomodoro, task, reasons, promoted[]}`
- reason kinds on `marked_blocked` / `unblocked`:
  `local_dependency`, `local_schedule`, `project_dependency`,
  `project_schedule`
- `prj_is_blocked_tags_added`, `prj_is_blocked_tags_stripped`
- `unblocked_fields_written`, `unblocked_fields_pruned`

Additive `bob projects sync` output:

- `hoisted_project_schedules`

A `project_*` reason's project path may name an ancestor that owns
the reason, not the note the blocked task lives in.

Warning kinds:

- `project_internal_dependency`
- `project_parent_cycle` (once per parent cycle, naming every member)

### 13.10 Gestures on `^prj`

Ctrl+Shift+P → Depends on on `^prj` (and a Task Link to `^prj`) edits
its Depends-On line plus resident side effects. One write: the line,
the `^prj` `[dependsOn::]` field, set `^prj` and every open resident
to `[?]` when any target is open, tag residents `#prj_is_blocked`,
consume their live links, transfer commitment, and — only when `^prj`
itself was `[?]` before the gesture — recover the tagged residents
with no remaining reason when the last reason clears (strip the tag;
write `[unblocked::]` when freshness.md §5 says to). Removing a link
from an unblocked `^prj` leaves untagged residents and their own
reasons alone. Ctrl+D on the Depends on row deletes the line and the
field.

After planning an edit to N's `^prj` through Depends on, the Task
Card runs the subtree pass: when N is blocked before or after the
gesture (own reasons, or N's `^prj` carrying the tag), it walks N's
descendants top-down — `blockedAfter(K)` is K's own reasons or its
parent's blocked-after state — and per descendant `^prj` writes the
tag ⇔ inherited, `[?]` while blocked (a tagged `[?]` `^prj` with no
remaining reason recovers to its derived rank), and per resident the
tag ⇔ blocked, `[?]` while blocked (tagged residents with no own
reason recover with `[unblocked::]` per freshness.md §5); live Task
Links of newly Blocked tasks under today's open Pomodoros are
consumed, and commitment transfers the strongest newly-blocked lane
(never onto a `^prj` target). Write order is N's note, then each
descendant note preimage-checked (a stale note is skipped and
reported), then the daily Pomodoro cleanup. A gesture on a tagged N's
`^prj` never recovers anything: it only re-asserts the descendants'
tags. Ctrl+Z undoes N's edit only; the next reconcile re-derives the
descendants.

Schedule on `^prj` writes the inline `[scheduled::]` field and runs
the same subtree pass for a future date, a due date, and a delete; a
tagged `^prj` keeps its tag through every Schedule write. Conversion
is line ↔ line both ways. Capture `&` on `^prj` writes the line.
`Alt+]` / `Alt+[` on a `[?]` `^prj` retires its own future date; when that
leaves the project with no remaining reason, the same gesture strips
`#prj_is_blocked` from every open resident and recovers those whose only
reason was the project, writing `[unblocked::]` when freshness.md §5 says
to. A remaining reason is a future inline date or a Depends-On id that
resolves to an open task; a missing id never blocks (DC7). Cycling `^prj`
out of `[?]` to any non-Blocked status — including Alt+[ to Cancelled,
which leaves the note no longer `project(T)` — recovers the same way, and
a transcluded `^prj` recovers residents in its source note. Closed lines
are never edited.

`create-project-note-from-task` (Ctrl+Alt+Shift+N) keeps the source's
Depends-On line as the first child of `^prj` and its inline date on `^prj`
on the way forward (rebase `[[#^a]]` → `[[source#^a]]`; never seed that line
as a Tasks row; skip the machine-owned Sub-projects child). The new
project lands blocked only for a carried open prerequisite or a future
inline date — a `[?]` source alone seeds Ready. Commitment transfers
the residents' pre-seed lanes. On reverse, the line becomes the
restored task's first child and the date stays inline on the
root task (links re-formed relative to the destination). `[?]` sources are
accepted. Residents blocked by the project are tagged or recovered through the
shared recovery gate; entering a blocked project consumes their live Pomodoro
links in today's open entries. Reversal writes the parent first, planning
`#^prj` rewrites and today's Pomodoro cleanup against both the project's
`^prj` link and the restored block id in memory before any write. A
rewrite that throws or finds changed content after the parent write keeps
the project note and is reported (`... was kept`, with the Pomodoro
suffix when the daily rewrite failed).

Ctrl+Shift+M into a blocked project sets the moved open tasks `[?]`,
tags them `#prj_is_blocked`, promotes only open tasks' Depends-On
targets (each to that task's own lane; the destination `^prj`'s
prerequisites to the strongest moved lane), walking through a `^prj`
target to its open prerequisites without raising it, and removes their
live links from today's open Pomodoros in the same move write; moving
them out strips the tag and recovers those without own reasons, writing
`[unblocked::]` when required by the freshness gate. Move, inbox-route,
and reversal notices report the blocked and recovered counts
(`· N Blocked · M Ready again`).

Capture `&` aimed at `^prj` writes the Depends-On line. Capture of a
new task into a blocked project lands `[?]`, `#prj_is_blocked`, and
unlinked. `@route:id s:N` with N ≥ 1 creates the task unlinked.

Writers other than the Task Card never cascade: capture `&` on
`^prj`, close recovery, Alt+`]`/Alt+`[` on a `[?]` `^prj`,
Ctrl+Shift+M, conversion, and new child projects (capture `+`,
Ctrl+Shift+Alt+N). Each one still respects the tag proxy, so it never
contradicts the cascade, and descendants catch up at the next
reconcile. Closes and Alt+`]`/Alt+`[` that unblock a project do not
recover its sub-projects at once: descendants keep their tags until
reconcile re-derives them.

### 13.11 Rollout

An old `bob projects sync` on the MacBook cron (`:05/:20/:35/:50`)
**deletes** inline `^prj` dates. So:

1. bob-cli on the MacBook first;
2. then this host;
3. then plugins on the MacBook.

Obsidian does not run on this host, so plugin phases deploy here as
usual. Phases never run a live `bob task reconcile` or a live
`bob projects sync`. Dry runs are always fine.

### 13.12 PDR — reconcile vectors

Each vector names the rule under test. `^prj` uses plain R1–R10.

| # | rule | situation | outcome |
| --- | --- | --- | --- |
| PDR4 | R1 | well-formed Depends-On line on open `^prj`, two resolved open targets | `[dependsOn::]` set to both ids in line order; targets stamped; same as DR1 |
| PDR7 | internal | `^prj` Depends-On link to a task in its own note | kept on the line; not projected into `[dependsOn::]`; warn `project_internal_dependency` |
| PDR8 | R7 | inherited cycle via a resident → `^prj` edge | keep; warn `dependency_cycle` with the path |
| PDR9 | scope | closed (`[x]`) `^prj` with a stale Depends-On line | untouched (DR18) |
| PDR10 | idempotence | second run over a reconciled vault | zero changes (except the daily-note exception in §13.5) |

### 13.13 PDB — blocking vectors

| # | situation | outcome |
| --- | --- | --- |
| PDB1 | `^prj` has one open Depends-On target; note holds a `#ref` task, a nested `#task`, and a fenced `#task` example | `#ref` and nested residents `[?]` with `#prj_is_blocked`; fenced line unchanged |
| PDB2 | same project | `^prj` is `[?]` with `local_dependency` only (never `project_dependency`) |
| PDB3 | last open target closes; a resident still has its own future `scheduled` | that resident stays `[?]` (own reason); others recover to derived rank; `#prj_is_blocked` stripped from all |
| PDB4 | `status: done` (or `canceled`) project with a still-open `^prj` prerequisite | not `project(T)`; residents are not inherited-Blocked; tag stripped |
| PDB5 | child project `parent: "[[blocked-parent]]"`; child has no own project reasons | child `^prj` and residents inherit: Blocked and tagged; see PDS1 |
| PDB6 | two open `^prj` lines in one note | not `project(T)` |
| PDB7 | `^prj` `[scheduled::]` strictly later than today; no open Depends-On target | `^prj` `[?]` with `local_schedule`; residents `[?]` with `project_schedule` and `#prj_is_blocked` |
| PDB8 | canonical daily note typed `project` with a `^prj` Depends-On line | ignored; not `project(T)` |
| PDB9 | ADJ-11: a dependent's open prerequisite is another project's `^prj`, which itself depends on an ordinary task | the `^prj` target is never raised into a lane; the walk continues and raises that project's open Depends-On targets; residents of the depended-on project are not promoted |

### 13.14 PDL — ledger vectors

Only today's open Pomodoros. `promoted[]` is the consumption seed.

| # | situation | outcome |
| --- | --- | --- |
| PDL1 | live link to a Blocked task under an open entry that has other children | link consumed; prerequisites promoted; entry kept (`consumed_blocked_references`) |
| PDL2 | that consumed link was the entry's last child | entry deleted (empty-Pomodoro rule; a running timed entry may go) |
| PDL3 | the target's last reason clears in the same run | link kept (final-state test) |
| PDL4 | closed entries in the same day file | byte-identical |
| PDL5 | struck link under an open entry | untouched |
| PDL6 | prose bullet whose only Task Link token is the Blocked target | token removed; the rest of the bullet kept |
| PDL7 | planned link-only bullet with authored child notes | whole subtree removed |

### 13.15 PDG — gesture vectors

| # | gesture | outcome |
| --- | --- | --- |
| PDG1 | plugin link gesture (Ctrl+Shift+Enter or `^^`) on a dependency-blocked target, or capture when §13.5.1 refuses a drop | not linked; open prerequisites (own plus inherited) promoted; reported |
| PDG2 | same link intent through `!`, Pomodoro-link insertion, Ensure Next, or link-then-close; only the target's own future `scheduled` blocks it | pulled forward as today and linked |
| PDG3 | same toggle; only a project schedule blocks a resident | refused; project date reported; not linked |
| PDG4 | `@route:id s:N` with N ≥ 1 | created unlinked |
| PDG5 | new task captured into a blocked project | `[?]`, `#prj_is_blocked`, unlinked, `not_linked` with the project reasons |
| PDG6 | link toggle on a `^prj` whose only reason is its own future `scheduled` | refused; "move the project date from the `^prj` Schedule action"; never a pull-forward |
| PDG7 | capture `&` on `^prj` adding an open target | residents `[?]` + `#prj_is_blocked`; their live links consumed; commitment transferred onto the open prerequisite (never onto a `^prj` target) |
| PDG8 | Task Card Depends on adds an open target to an unblocked, untagged N's `^prj`. N has child K (2 residents) and grandchild G | N's residents, plus K's and G's `^prj` and residents, become `[?]` + tag. Their live links in today's open Pomodoros are consumed. The target rises to the strongest of their lanes. Notice `· 2 sub-projects Blocked` |
| PDG9 | removing N's last open target (N untagged, `^prj` `[?]` before) | K and G recover: tags stripped, and tagged lines with no own reason return to their derived rank with `[unblocked::]` per §5. Notice `· 2 sub-projects Ready again` |
| PDG10 | as PDG9, but K's `^prj` has its own open target | K's `^prj` loses the tag and stays `[?]` (`local_dependency`). K's residents keep `[?]` + tag (reason names K). G keeps its tags (reasons name K) |
| PDG11 | any Depends on gesture on a tagged N's `^prj` (N inherits from M) | nothing recovers. Descendants' tags are only re-asserted |
| PDG12 | Task Card Schedule writes a future date on an unblocked N's `^prj`; then a due date, or Ctrl+D on the date | like PDG8 (with `project_schedule`); then like PDG9. A tagged `^prj` keeps its tag through every Schedule write |
| PDG13 | Depends on stage on N's `^prj`, or capture `&` on it, targeting a task in a descendant | refused: "inside a sub-project of this project" |
| PDG14 | a descendant note changed after the pass read it | that note is skipped and N's write stands. Notice suffix `· 1 sub-project not updated`. The next reconcile repairs it |
| PDG15 | a link gesture (capture link guard, `^^` / Ctrl+Shift+Enter in block-id-prompt, Pomodoro link insertion) on K's `^prj` or a K resident while K's `^prj` is tagged | not linked. The `project` reason names the owning ancestor (capture) or `parent(K)` (plugins). Capture promotes the open prerequisites of each ancestor up to and including the first untagged one |
| PDG16 | a new task captured into K while K's `^prj` is tagged | lands `[?]`, `#prj_is_blocked`, unlinked, `not_linked` with the project reason |
| PDG17 | Alt+] / Alt+[ on K's tagged `[?]` `^prj` | refused, as for a tagged resident: `Project blocked by parent project · <parent(K)>` |
| PDG18 | capture `@route:id` (also `^route:id`, `#name`, `=<X>`) on a target whose only reason is `local_dependency` | open prerequisite links dropped from the line and field; `[?]` → `[*]`; linked (and started for `=<X>`); `dropped_dependencies` listed; prerequisite notes byte-identical |
| PDG19 | the same target through `@route+id!`, `@route+id` / `@route+id#name` Ensure Next, and `@route:id=x…` | dropped, then linked, relocated, or linked-and-closed exactly like an unblocked target (`=x1` leaves it `[/]`, `=x!1` completes it) |
| PDG20 | the dropped links were the line's only links | line deleted; `[dependsOn::]` removed |
| PDG21 | own open prerequisites plus a single own future `scheduled` | both retired in one write (PDG2 pull-forward plus the drop); linked |
| PDG22 | the line also holds a Done prerequisite and an unresolved link | the Done and unresolved links are kept verbatim in order; their field ids are kept |
| PDG23 | own open prerequisites, and the task is a resident of a blocked project (own or inherited reason) | nothing dropped; PDG1/PDG15 consumption; `drop_refused: project_blocked` |
| PDG24 | a `^prj` target with an open prerequisite | nothing dropped; PDG1 consumption; `drop_refused: project_task` |
| PDG25 | two managed Depends-On lines, or several `scheduled` fields | nothing dropped; consumption; `drop_refused: dependency_line` or `schedule` |
| PDG26 | `^route:id=` while a timed Pomodoro runs, or a later item in the batch fails | the batch fails; nothing is dropped; the dry run reports the same failure |
| PDG27 | plugin Ctrl+Shift+Enter or `^^` on a dependency-blocked target | unchanged PDG1; plugins never drop |

### 13.16 PDC — close vectors

Outcomes of the successor-link rule (§12.2) once project reasons exist.

| # | situation | outcome |
| --- | --- | --- |
| PDC1 | dependent D lives in a still-blocked project; this close removed D's last *own* prerequisite | D stays `[?]`; `still_blocked{reason: project}`; no successor link |
| PDC2 | this close fully unblocks a `^prj`; residents have no own remaining reason | `^prj` recovers (`not_linked: project_task`); residents recover-only (never successor-linked), lose `#prj_is_blocked`, and receive `[unblocked::]` when freshness.md §5's WRITE rule applies; those recoveries are outside the breaker (except §12.10) |
| PDC3 | same as PDC2, but one resident still has an open prerequisite of its own | that resident stays `[?]`; `#prj_is_blocked` is still stripped |
| PDC4 | dependent D is a resident of K (K's `^prj` tagged); this close removes D's last own prerequisite | D stays `[?]`; `still_blocked{reason: project, project: parent(K)}`; no link |
| PDC5 | K's tagged `^prj` loses its last own prerequisite | stays `[?]`; `still_blocked{reason: project}`; no `project_task` recover; no PDC2 / PDC3 for K's residents |
| PDC6 | a close fully unblocks an untagged N that has tagged descendants | descendants keep their tags until the next reconcile, which recovers them (PDS4 / PDS5 outcomes). PDC2 covers only N's own residents |

### 13.17 PDF — Databricks fixtures

Use these two vault states as end-to-end fixtures. Adapt descriptions
if the notes have been edited since. The Depends-On child stays; there
is no R0 hoist.

**PDF1 — pre-06:57 (links still in the ledger).**
`job_apply_to_databricks.md` has a Depends-On child
`[[job#^update-linkedin]]` on a `[?]` `^prj` with
`[dependsOn:: job__update-linkedin]`, and two `#ref` tasks `[*]`
under `### Next & In Progress`. Both `#ref` Task Links sit in the
open timed `0645-0710 — JOB` Pomodoro, which has other children.

After one reconcile:

- the Depends-On child stays;
- `[dependsOn:: job__update-linkedin]` remains on a `[?]` `^prj`;
- both `#ref` tasks are `[?]` with `#prj_is_blocked` (PDB1);
- exactly those two links are consumed (PDL1); the JOB entry is kept
  because other children remain;
- LinkedIn (the prerequisite) is promoted;
- a second run changes nothing (PDR10).

A sibling fixture of PDF1 whose blocked links are the last children of
their entry expects that entry deleted (PDL2).

**PDF2 — current (links already removed by hand).** Same project note
and field, no live `#ref` links under today's open Pomodoros.

After one reconcile: the inherited Blocked and tags of PDF1, and
**no** ledger change (`consumed_blocked_references` empty for those
two tasks). `job.md`'s LinkedIn task stays `[?]` from its own
dependency. `job_update_cv`'s `^prj` is not raised (PDB9).

### 13.18 PDT — `#prj_is_blocked` tag vectors

`tag_scope = every_resident`: every open resident of a blocked
project carries the tag.

| # | situation | outcome |
| --- | --- | --- |
| PDT1 | project becomes blocked (open projected prereq or future `^prj` scheduled); note holds open residents | each open resident gains exactly one `#prj_is_blocked`; `^prj` does not |
| PDT2 | project recovers (last reason gone) | tag stripped from every open resident |
| PDT3 | resident moved out of a blocked project | tag stripped from the moved task |
| PDT4 | project becomes terminal (`status: done` / `canceled`) | tag stripped from every open resident |
| PDT5 | hand-copied `#prj_is_blocked` on an open task that is not a resident of a blocked project | stripped |
| PDT6 | `#prj_is_blocked` on an open `^prj` that inherits no reason | never added; if present, stripped |
| PDT7 | closed (`[x]` / `[-]`) line carrying `#prj_is_blocked` | untouched (history) |
| PDT8 | resident already tagged, project still blocked | exactly one tag (no duplicate) |
| PDT9 | placement | tag lands after the fields and `#hide`, before `^id` (`task_metadata_insertion_offset`) |
| PDT10 | CRLF preserved | tag edit uses the note's line endings |
| PDT11 | resident with its own open prerequisite, in a blocked project | still tagged `#prj_is_blocked` (and stays `[?]` for both reasons) |
| PDT12 | token-bounded matching | `#prj_is_blocked` is never matched as `#prj`; `#prj` is never matched as `#prj_is_blocked` |
| PDT13 | an open `^prj` that inherits a reason | tagged exactly once; placement as PDT9 |

### 13.19 PDS — sub-project cascade

Outcomes of the cascade rule (§13.3) in reconcile. Reconcile resolves
`parent:` for every recognised project from the scan it already holds,
so the cascade costs no extra file reads.

| # | Situation | Outcome |
| --- | --- | --- |
| PDS1 | N's `^prj` has one open target X. Child K (`parent: "[[N]]"`) has no own reason; its residents are a nested `#task` and a `#ref` task | K's `^prj` → `[?]` + `#prj_is_blocked`, with reason `project_dependency{project: N, open_ids: [X]}`. Both residents → `[?]` + tag, with the same reason. N's `^prj` stays untagged |
| PDS2 | grandchild G of K (3 levels) | G's `^prj` and residents are Blocked and tagged; their reasons name N |
| PDS3 | N's `^prj` has a future inline date and no open target | descendants get `project_schedule{project: N, date}` |
| PDS4 | X closes | K and G lose every tag. Each line returns to its derived rank. `[unblocked::]` is written per freshness.md §5 (a `#hide` `^prj` is skipped) |
| PDS5 | as PDS4, but K's `^prj` also has its own open target Y | K's `^prj` loses the tag but stays `[?]` (`local_dependency`). K's residents keep `[?]` + tag (reason names K). G keeps its tags (reasons name K) |
| PDS6 | K's parent is an area note, a terminal project, a two-`^prj` note, a canonical daily note, unresolvable, an ambiguous basename, a list-form `parent:`, or K itself | no inheritance. A hand-copied tag on K's `^prj` is stripped |
| PDS7 | N → K → G with K `status: done` | G inherits nothing from N (the chain stops at the first non-project) |
| PDS8 | N's `^prj` Depends-On line links a task in K or G | the link is kept on the line, never projected into `[dependsOn::]`, and warns `project_internal_dependency`; it blocks nobody |
| PDS9 | parent cycle A ↔ B; A's `^prj` has an open target | B inherits from A; A gets no reason from itself; `project_parent_cycle` warns once, naming both |
| PDS10 | live Task Links to K's `^prj` and to a K resident under today's open Pomodoro | both are consumed. X is promoted through the ancestor edges (ADJ-11 still never raises a `^prj` target). `consumed_blocked_references` lists the reasons |
| PDS11 | Y depends on a K resident while N's `^prj` depends on Y | R7 warns `dependency_cycle` through the implicit ancestor edge |
| PDS12 | second run over a reconciled tree | zero changes |
| PDS13 | K's `^prj` was `[*]` and linked today when N became blocked | `[?]`; the link is consumed; the lane is lost |
| PDS14 | `bob projects sync --dry-run` on a cascade-blocked tree | no change caused by the tags: descriptions drop the tag, marker states are unchanged, and a `[?]` child still counts as an open sub-project |
| PDS15 | `parent: "[[N\|Alias]]"`, `"[[dir/N]]"`, `"[[N#Heading]]"` | all resolve to N |
