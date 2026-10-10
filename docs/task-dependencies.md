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
- **Closing.** Closing a planned prerequisite links the dependents it
  fully unblocked into its slot (§12).

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
| SL11 | D is `^prj` / carries `#hide` | recover only; `project_task` / `hidden` |
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
the predecessor's slot and becomes Next in the same write.

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

It **never** runs on any of the following:

- cancel (the nav Ctrl+Shift+P cancel, Alt+[ into Cancelled);
- reopen;
- raw checkbox clicks;
- `bob task reconcile` / hooks;
- `=*` parks;
- Ctrl+Enter on a Depends-On line.

Cancels still recover dependents exactly as today.

### 12.2 Rule

One definition, two engines (Rust capture and the JS cycler):

```text
C        = tasks this gesture moved open → Done (roots + closed embedded subtasks),
           read from the staged post-close text
GATE     : if no c ∈ C carries an [id::] field → stop. No dependents lookup and no extra
           reads. unblocked_check = "checked"; unblocked = still_blocked = [].
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
  3. D's block id is prj          → recover; not_linked: project_task
  4. D carries #hide              → recover; not_linked: hidden
  5. D had a live link before this gesture
                                  → recover; not_linked: already_planned
  6. no predecessor of D in C has an anchor
                                  → recover; not_linked: not_planned_today
  7. plan.link_unblocked is false → recover; not_linked: disabled
  8. otherwise                    → SUCCESSOR, anchored at the earliest (ledger order)
                                    anchor among D's predecessors in C

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
  "reason": "scheduled", "waits_on": 0, "scheduled": "2026-10-13"}],
"unblocked_check": "checked"
```

**`unblocked[]` rows**

- The existing fields keep their meaning; `unblocked` stays on
  `task_complete` and is new on `pomodoro_close`.
- `block_id` holds the minted ID when one was minted, and `""` when D
  has none and none was minted.
- `link` is non-null exactly when this gesture linked D.
  - Line numbers are 1-based in the day text after this item.
  - `day_file` is vault-relative.
  - `entry_name` is `""` when the entry is unnamed.
- `not_linked` is one of `already_planned`, `not_planned_today`,
  `breaker`, `project_task`, `hidden`, `disabled`.
  - The cycler-only model adds `failed` and `cancelled`; Rust never
    emits them.

**`still_blocked[]` rows**

- `reason` is `waits_on` or `scheduled`.
- `waits_on` counts the open prerequisites left.
- `scheduled` is set only for `scheduled`, otherwise `null`.

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
  - The title is `Unblocked`.
  - The count chip reads `n → NAME` / `n → new NAME session` /
    `n linked`, plus `· next up` when it applies.
  - The right-aligned muted receipt reads `✓ <predecessor>`, or
    `✓ N tasks`.
- **Rows.**
  - **Linked rows** show `＋ text`, a lane pill (`Next`), `↗ <note>`
    when D lives in a different note than its first predecessor, and
    `inbox` when D lives in an inbox file.
  - **Not-linked rows** show the text, a lane pill (`Ready`/`Next`), and
    a muted reason: `already planned`, `not planned today`,
    `not linked · more than 5`, `project task`, `hidden`, `linking off`,
    `couldn't link`, or `cancelled`.
  - **Still-blocked rows** are muted: `🔒 text · waits on N more` or
    `· waits until Oct 13`.
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
primary surface because it explains before anything is written.

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
  still blocked  Start adding sase--<name> badges  sase_agents_repo.md ^badges · until 2026-10-13
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
- **Still-blocked suffixes:** `· waits on N more`, `· until YYYY-MM-DD`.
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
- each unblocked row reports `not_linked: "disabled"`.

Recovery is the honest half of the feature and costs nothing extra once
the dependents are found. An invalid value falls back to `true` and
surfaces through the existing invalid-plan-config warning paths
(`docs/plan.md` "Invalid values").

### 12.9 Undo

- **One successor.** Alt+N on its link releases it, as the inverse of
  what was written. The card's chip teaches this.
- **The whole gesture, in Obsidian.** Reopening the predecessor the same
  day takes back its successor links if they are untouched. This uses an
  in-memory receipt; nothing is stored in Markdown.
- **Not undo.** Deleting the bullet or pressing `u` in the editor leaves
  the sticky Next. That is the general sticky-lane cost.
