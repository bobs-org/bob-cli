# Task dependency links

A task's prerequisites live as plain task dependency links on one
managed `⛓️ **DEPENDS ON:**` first-child line. That line is the source
of truth; the `[dependsOn::]` / `[id::]` fields are derived from it.

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
  dependent's Depends-On line.
- **Depends-On line**: the single managed first-child line that holds a
  task's dep links.
- **Dependent**: the task that owns the line.
- **Prerequisite**: a link target.
- **Blocked** stays the name of the derived `[?]` state.

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
  - to the right of any `[fresh::]`, because the hooks'
    trailing-field parser stops at `fresh`;
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
block id).

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
  finish-to-start) or while its `scheduled` date is in the future.
  Done and Cancelled prerequisites stop blocking.
- **Promotion.** Pomodoro roots promote their prerequisites to
  Next/In Progress along the reconciled set: line links plus R8 legacy
  children. The edge rules are unchanged: strongest rank wins, the walk
  is cycle-safe, recovery uses the same edges, and the lanes are
  sticky.
- **Not edges:** `#^ref` embeds and archived targets.
- **Closing.** Closing a dependent **never** closes its prerequisites.
  Embedded-tree closes in capture `=x` and the cycler ignore
  Depends-On lines.
- **History.** Closed prerequisites stay on the line as history and
  show as muted chips. Prerequisites never prevent closing a dependent
  by hand.
- **Adding a dependency** sets the dependent to `[?]` if the target is
  open. If the dependent was `[*]` or `[/]`, an open target rises to
  at least that lane (`getDependencyPromotionStatus`).
- **Removing a dependency** recovers the dependent to its derived rank
  _immediately_ when no open prerequisite and no future `scheduled`
  date remain. Otherwise it stays `[?]`. The hooks keep the final word.

## 6. The Depends on stage (navigation-hotkeys)

### 6.1 Entry points

| Cursor | Gesture | Edits |
| --- | --- | --- |
| A `#task` line | Ctrl+Shift+P then `b` on the Task Card (Blocked by) | this task |
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
path in a tooltip; a target with no `[id::]` whose path can't be
encoded; a target or dependent that changed since the stage opened
(refuse, then reopen fresh). Removing a link is always allowed.

### 6.5 Block ids

A `＋ id` row opens the existing block-ID stage when the change is
applied. It is pre-filled from `suggestBlockIdFromTask`, with
uniqueness checked against the **target's** note, so ↵ accepts it. A
batch prompts for one target at a time. Highlighting a row never
writes.

### 6.6 One gesture writes

1. **Prepare cross-note targets first.** Add the `^id` and `[id::]`
   through the open editor if the note is open, otherwise with a
   preimage-checked `vault.process`.
2. **Commit the dependent's note** in **one editor transaction** (one
   Ctrl+Z): the line, the field, same-note target ids, status effects,
   folding of legacy children, and `[fresh:: today]` via
   `api.freshness.stampLine`.
3. **If preparation fails**, the dependent is untouched. An unused
   target id is acceptable; a link to an unprepared target is not.

### 6.7 Notices

- `⛓ Now waits on "File for unemployment" · Blocked`
- `⛓ No longer waits on "Launch swarm…" · Ready again`
- `⛓ 2 added · 1 removed · Blocked (1 open)`

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
| `bob capture` `&note:id` (and the Bob Mac Capture `&` picker) | Adds prerequisites to a new task or an explicit `@note+id` dependent through the staged capture writer: new links append in typed order (repeats are no-ops), legacy children fold and field-only dependencies are adopted, fields are derived, and §5 Blocked/promotion applies in the same batch. It never removes or reorders links; removal stays with Ctrl+Shift+P and Ctrl+D. See `docs/capture.md`. |
| Ctrl+Shift+M, `task archive` | The line moves with its task. Same-note links inside a moved block whose target stayed behind gain the source note path. |
| "Rewrite dependency navigation links" command, `migrate-dependency-bullets.mjs` | Deleted. They emit embeds. |

## 9. Plugin api v1 (navigation-hotkeys)

```js
app.plugins.plugins["bob-navigation-hotkeys"].api = Object.freeze({
  version: 1,
  openDependencyStage(ref),            // ref: { path, line } — any line of the task block or its Depends-On line
  removeDependency(parentRef, target), // target: { path, blockId }
});
```

- Both members return Promises resolving to `{ ok, reason? }` and
  never throw.
- `ref.line` and `parentRef.line` are 0-based line indexes into the
  note's lines (the same indexing the editor and `findOwningTaskLine`
  use): the Depends-On line itself, or any line of the owning task
  block. Non-integer or out-of-range lines refuse with `invalid-ref` /
  `line-out-of-range`.
- `removeDependency` re-reads the dependent and refuses with a notice
  when it is stale, and refuses with `not-on-line` (never a silent `ok`)
  when the target is not on the dependent's line.
- Plugins never import each other's `main.js`. bob-ledger-tools
  feature-detects `api?.version >= 1`.

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

## 11. Conformance vectors

Implementations copy the tables they need into their tests, citing the
section in a comment. `line` below is the candidate Depends-On line on
its own; `context` names what surrounds it. `verdict` is one of
`accept(n)` (a line with n targets), `empty` (label but no links),
`malformed`, or `not-a-line`.

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
