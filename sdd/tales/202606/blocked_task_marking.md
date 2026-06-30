---
create_time: 2026-06-30 09:30:16
status: done
prompt: sdd/prompts/202606/blocked_task_marking.md
---
# Plan: Mark dependency-blocked tasks in the `^^` task-link picker

## Product context

The **Block ID Prompt** Obsidian plugin (`plugins/block-id-prompt/` in the `bob-plugins` repo) powers the `^^` trigger:
typing `[[foobar^^]]` opens a "Link to task" picker (`TaskLinkPickerModal`) listing every **open** `#task` in the target
note (`~/bob/foobar.md`), so the author can link a wiki block reference to a task and auto-create/reuse its block id.

Today the picker shows, per row: a literal status pill (`[ ]` / `[/]` / `[B]`), the task title, a `Line N` meta line,
and a link/create badge. It says nothing about **dependencies**.

In this vault a task dependency is expressed with the inline field `[dependsOn:: <id>, ...]` whose comma-separated
values are the **bare ids** of the tasks it waits on. A task's id is its `[id:: <value>]` field when present
(canonical), otherwise its trailing block id `^<value>` (see the `dependson-block-id-convention` and the
`bob-navigation-hotkeys` DEPENDS-ON authoring flow, which create _local_, same-note dependencies). A task is effectively
**blocked** when at least one task it depends on is not yet done.

**Goal:** make the picker visually mark which listed tasks are blocked by an unfinished dependency — intuitively
(instantly scannable), reliably (correct against the real `dependsOn` convention, no false positives, zero overhead when
unused), and beautifully (a restrained amber chip + accent that fits the existing modal language and yields to selection
state).

This is a UI/UX enhancement to one Obsidian plugin. It adds **no CLI subcommands**, no new dependencies, and no build
step (these are plain CommonJS plugins — `main.js` is the source).

## Definitions (the "blocked" semantics)

- **Dependency value** — a bare token inside `[dependsOn:: a, b, c]`: split on `,`, trim whitespace, drop empties,
  dedupe (order preserved). Multiple `[dependsOn::]` fields on one line are all collected.
- **Task id-key** — how a candidate target task is matched by a dependency value. A task line is keyed by **both** (a)
  its `[id:: <value>]` field value (canonical, when present) and (b) its trailing block id `^<value>`. A dependency
  value matches a task if it equals either key. (Indexing both is what makes resolution robust against the case where a
  pre-existing `[id::]` value differs from the block id.)
- **Done status** — checkbox char in `{x, X, -}` (completed + cancelled). These **satisfy** a dependency. Every other
  status (` `, `/`, `B`, etc.) is still **blocking**. (Matches Obsidian Tasks, which unblocks on DONE or CANCELLED.)
- **Blocking dependency** — a dependency value that resolves, within the same note, to a task whose status is not done.
- **Blocked task** — an open task in the list with **≥1 blocking dependency**.
- **Unresolved dependency** — a dependency value with no in-note id-key match (a cross-note or stale reference). It does
  **not**, by itself, mark a task blocked (avoids false positives on references we cannot verify); it is surfaced only
  in the hover tooltip.

### Resolution scope (and why)

Dependencies are resolved **within the target note only** — the note's full content is already loaded by the picker
(`destination.content`), block ids are note-local in Obsidian, and the DEPENDS-ON authoring flow creates local
dependencies. So same-note resolution is both reliable and free (no vault scan). Cross-note dependencies are an
explicit, documented out-of-scope item for this iteration; they degrade gracefully (treated as "unresolved", never a
false "blocked").

## High-level technical design

All work is in `plugins/block-id-prompt/`: `main.js` (logic + render) and `styles.css` (presentation). The plugin
already parses tasks, strips inline fields from display text (`TASKS_INLINE_FIELD_RE` removes `[dependsOn:: …]` from
titles — so the only new on-screen element is the chip), and skips frontmatter/code fences — all of which we reuse.

### 1. Parsing & resolution (pure, testable helpers in `main.js`)

Add near the existing task constants:

- `DONE_OBSIDIAN_TASK_STATUSES = new Set(["x", "X", "-"])`.
- `INLINE_ID_FIELD_RE` and `INLINE_DEPENDS_ON_FIELD_RE`, mirroring the patterns already proven in `task-status-cycler`
  (capture the field value; `dependsOn` is global to catch repeats).

Add pure functions:

- `parseInlineIdField(lineText)` → the `[id::]` value (trimmed) or `null`.
- `parseDependsOnIds(lineText)` → deduped array of bare dependency ids.
- `taskIdKeysFromLine(lineText)` → `[idFieldValue, trailingBlockId]` filtered to non-empty and deduped (id field first =
  canonical).
- `isDoneTaskStatus(status)`.
- `buildTaskDependencyIndex(content)` → one pass over **all** task lines (any status, with or without `#task`), reusing
  the same frontmatter/code-fence skipping as `getOpenTasksInContent`. Returns a
  `Map<idKey, { status, done, title, line }>` (title via the existing `cleanTaskDisplayText`). Building from _all_
  statuses is essential: a blocker that is already done is absent from the open-task list but must still resolve to
  "satisfied". On a key collision (duplicate ids — defensive; ids should be unique per note), the **open** entry wins so
  a real blocker is never masked.
- `resolveTaskDependencyState(rawLine, index)` → returns
  `{ depIds, unmetBlockers: [{id, title}], metCount, unresolvedIds, isBlocked }` where
  `isBlocked = unmetBlockers.length > 0`.

### 2. Enrichment wiring

- Add `collectTaskPickerItems(content)`: build the dependency index once, take the existing
  `getOpenTasksInContent(content)` list, and attach `task.dependency = resolveTaskDependencyState(task.rawLine, index)`
  to each.
- In `openTaskLinkPicker`, replace the `getOpenTasksInContent(destination.content)` call with
  `collectTaskPickerItems(destination.content)`. No other call sites change; `getOpenTasksInContent` stays as-is for
  reuse. (Two linear passes over a single note is negligible.)

### 3. Rendering (`TaskLinkPickerModal`)

- In `renderTaskRow`, after the `Line N` meta span, when `task.dependency.isBlocked`:
  - add the class `is-dep-blocked` to the row;
  - append a **blocked chip** into the meta line: a `lock` icon (via the existing `applyIcon`)
    - label `Blocked`, or `Blocked · N` when more than one dependency is unmet;
  - set the chip's `aria-label` (native Obsidian tooltip + screen-reader text) to a summary built by a small
    `buildBlockedTooltip(dependency)` helper: `"Blocked by: <title1>, <title2>"` using cleaned blocker titles, truncated
    to the first ~3 with `+N more`, and a trailing note when there are unresolved ids (e.g. `; N unresolved`).
- The chip lives on the **secondary (meta) line**, keeping the title row clean and the signal scannable. The literal
  status pill is unchanged: `[B]` remains the _author-set_ status, while the chip is _computed_ from `dependsOn` — the
  two can legitimately coexist and that is honest.
- `updateSubtitle()`: when not actively filtering, append ` · N blocked` to the existing summary when `N > 0` (e.g.
  `12 open tasks in foobar · 3 blocked`) for an at-a-glance count; in the filtered ("Showing X of Y") branch, append the
  blocked count among the visible set when > 0.
- Blocked tasks remain **fully selectable/linkable** — the marking is informational, never a gate (you often link
  precisely to the thing you're waiting on).

### 4. Presentation (`styles.css`)

- Make `.bid-tlp-row-meta` a wrapping flexbox (`display:flex; gap; flex-wrap:wrap`) so `Line N` and the chip share a
  line and reflow gracefully (including the ≤700px layout, where the chip stays in the text column and is unaffected by
  the badge reflow).
- Add `.bid-tlp-blocked-chip` (+ `.bid-tlp-blocked-icon`): a compact pill in an **amber/warning** accent built on the
  stable Obsidian `--color-orange` variable via `color-mix` for border/background (matching how existing badges/pills
  tint accents). Amber is deliberately distinct from the **red** `--text-error` used by the `[B]` status pill (status ≠
  computed block) and from the blue interactive accent. `cursor: help` signals the tooltip; a 12px icon.
- Add a subtle scannable row stripe that **yields to selection**: `.bid-tlp-row.is-dep-blocked:not(.is-selected)` gets a
  faint amber `border-left-color` (reusing the row's existing 3px left border channel), while `.is-selected` keeps the
  accent stripe. Do **not** mute/disable the title — blocked rows are still actionable.

## Edge cases (handled by the above)

- All dependencies done → not blocked, no chip (no noise).
- Some met, some unmet (`[dependsOn:: a, b]`, a done / b open) → `Blocked · 1`, tooltip lists b.
- Unresolved-only deps (no in-note match) → no chip; surfaced only if combined with real blockers.
- `[id::]` value differs from block id → both keyed, matches either form.
- Multiple `[dependsOn::]` fields / duplicate ids on a line → all collected, deduped.
- `dependsOn`-looking text in frontmatter or code fences → skipped by the existing iterator.
- Self-dependency / cycles → only direct status is checked (no graph recursion), so always safe; an open self-dependency
  reads as blocked, which is acceptable.
- No Tasks plugin / no `dependsOn` anywhere → zero chips, identical UI, no measurable overhead (fully backward
  compatible).

## Versioning, docs, validation, deploy

- Bump `plugins/block-id-prompt/manifest.json` `version` `1.1.1` → **`1.2.0`** (additive feature).
- Update the `README.md` plugins table: block-id-prompt version → `1.2.0` and extend its description to mention "marks
  dependency-blocked tasks".
- **Validate:** `npm run validate` must pass (manifest checks + `node --check` syntax on `main.js`). There is
  intentionally no unit-test harness, and `main.js` cannot be `require()`d standalone (it imports `obsidian` at module
  load), so automated logic tests are not wired up.
- **Manual verification matrix** in a scratch `~/bob/foobar.md`, opening the picker via `^^`:
  1. Open task, no deps → no chip.
  2. `[dependsOn:: a]` where `^a` is open → `Blocked`, tooltip "Blocked by: <A title>".
  3. Mark `^a` done (`[x]`), reopen → chip gone.
  4. `[dependsOn:: a, b]` with `^a` done, `^b` open → `Blocked · 1`, tooltip lists only B.
  5. `[dependsOn:: nope]` (no match) → no chip.
  6. Task whose `[id:: custom]` differs from `^block` id, referenced by `custom` → resolves.
  7. Subtitle shows `· N blocked`; filtering still works; a blocked row still links on Enter.
  8. Visual pass: amber chip + faint left stripe; selecting overrides with the accent stripe; legible in light & dark
     themes; chip wraps at ≤700px width.
- **Deploy** (user-initiated rollout, from the `bob-plugins` repo checkout): preview then sync, then reload the plugin
  in Obsidian:
  ```bash
  bob plugins sync -p block-id-prompt -r "$PWD" --dry-run
  bob plugins sync -p block-id-prompt -r "$PWD"
  ```
  If sync reports the managed file "dirty in vault; use -F/--force", verify the on-disk vault file matches its committed
  baseline before forcing (see `bob-plugins-deploy-from-workspace`).

## Out of scope (possible follow-ups)

- Cross-note dependency resolution (would require a vault/metadata-cache scan).
- Sorting/grouping blocked tasks, or a "hide blocked" filter toggle.
- Transitive/"blocks N others" indicators or full dependency-graph visualization.
