---
create_time: 2026-06-28 09:48:00
status: wip
prompt: sdd/prompts/202606/obsidian_depends_on_local_task_picker_1.md
---
# Plan: `dependsOn` Property + `local_task_id` Value Type for the Ctrl+Shift+P Bullet Property Picker

## 1. Goal & Product Context

Add a new `dependsOn` bullet property to the Obsidian **"Set bullet property"** picker — the modal triggered by
`Ctrl+Shift+P` (command `set-bullet-property` in the `bob-navigation-hotkeys` plugin). Selecting `dependsOn` opens a
second stage listing **every open task in the current file**. Choosing a task records a dependency on it from the bullet
under the cursor.

Two things are guaranteed when a dependency is recorded, so the result is immediately usable by Obsidian **Tasks**
queries and by Obsidian's native block linking:

1. The **target task** is given a stable identity: a trailing block ID (` ^<id>`) and a matching inline `[id:: <id>]`
   field. If the task has no block ID yet, the user is prompted to create one (auto-suggested, editable); if it has a
   block ID but no `[id::]`, the `[id::]` is added automatically.
2. The **current bullet** receives `[dependsOn:: <id>]`, merged into any existing comma-separated dependency list.

This is the manual-authoring counterpart to a convention that already exists in the vault: the Obsidian **Tasks** plugin
emits `[id:: <id>]` on a target task and `[dependsOn:: <id>]` on the dependent task, and the **task-status-cycler**
plugin rewrites Tasks' random IDs to the target's block ID (SDD tale `task_dependency_block_ids`, status: done). Today
the only way to add a dependency is Tasks' own autocomplete. This feature gives a first-class, keyboard-driven,
beautiful picker that produces values **identical in shape** to what that pipeline standardizes on — so nothing
downstream has to change, and (unlike the existing draft's earlier stance) the `[id::]` is written eagerly so Tasks
queries resolve the dependency without waiting for any normalizer to run.

The `^^` "complete a wiki block link to an open task" flow in the `block-id-prompt` plugin is the explicit design
inspiration: same notion of "pick an open task in this file; mint a block ID if it lacks one." This feature improves on
it in three ways — it lives inside the existing polished picker (no separate dialog), it **auto-suggests** a block ID
slug (the `^^` prompt offers none), and it also writes the `[id::]` field for Tasks-query parity.

### Design north stars (intuitive, reliable, beautiful)

- **Intuitive** — reuses the exact two-stage flow users already know from the date/value picker, and adds a natural
  third stage **only** when a block ID must be created. Dependencies accumulate (re-run to add more) and already-linked
  tasks are visibly marked.
- **Reliable** — the written dependency value is the canonical identifier the whole ecosystem already understands
  (Tasks, task-status-cycler, Dataview), and `[id::]` + `^id` are kept in sync. All edits are line-local, addressed by
  line number, and guarded against the document changing mid-operation. Existing fields are never silently rewritten —
  only missing ones are added.
- **Beautiful** — the whole flow stays inside the existing `bob-cnp` modal (header icon, search box, status pills,
  badges, keyboard-hint footer). No second plain dialog pops up; the modal morphs through its stages, porting the
  `block-id-prompt` task-row visual language into the picker's `bob-cnp-*` class family.

## 2. Key Design Decisions (I am leading these)

| Decision                   | Choice                                                                                                                                                                                                                      | Rationale                                                                                                                                                                                                                                                                                                                                                                                            |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Dependency value**       | The target task's **identifier**: its existing `[id::]` value if it has one, otherwise its block ID (existing or newly minted). No `^`, no `[[ ]]`.                                                                         | Matches the `[dependsOn:: <id>]` convention produced by Tasks and normalized by task-status-cycler. In the normal case the block ID and `[id::]` are equal, so the value equals the block ID (the user's literal phrasing). Preferring an existing `[id::]` in the rare mismatch case keeps the dependency **functional in Tasks queries** — which is the stated reason for writing `[id::]` at all. |
| **Target `[id::]` write**  | Add `[id:: <id>]` to the target task when absent (positioned before the trailing block ID). Never overwrite a differing existing `[id::]`.                                                                                  | The user's additional requirement: Tasks resolves `dependsOn` against `[id::]`, not against the raw block token. Non-destructive (add-only) so we never corrupt user-authored IDs or fight the normalizer.                                                                                                                                                                                           |
| **Block-ID creation**      | Prompt **only** when the target has neither a block ID nor an `[id::]`. If it has an `[id::]` but no block ID, append ` ^<id>` automatically (no prompt). If it has a block ID but no `[id::]`, add `[id::]` automatically. | Honors "prompt if no block ID" while avoiding a needless prompt when a usable identifier already exists. Keeps the block-ID == id invariant the ecosystem relies on.                                                                                                                                                                                                                                 |
| **Block-ID prompt UX**     | A third **in-modal** stage (not a separate dialog), pre-filled with a slug auto-derived from the task text, editable, validated live, uniqueness-checked against the file.                                                  | One cohesive surface (beautiful); strictly better than `^^`'s bare prompt, which offers no suggestion.                                                                                                                                                                                                                                                                                               |
| **Block-ID placement**     | Appended as the final token ` ^id`, after any inline fields; `[id::]` inserted before it.                                                                                                                                   | Preserves the "block ID is the last token" invariant; yields `… [id:: x] ^x`, exactly the normalized shape.                                                                                                                                                                                                                                                                                          |
| **Multiple dependencies**  | Merge + de-dupe into the existing comma-separated list (append, never overwrite).                                                                                                                                           | `[dependsOn:: a, b]` is already supported upstream; re-running should _add_ a dependency.                                                                                                                                                                                                                                                                                                            |
| **Which tasks are listed** | Open tasks (`OPEN_OBSIDIAN_TASK_STATUSES` = `" "`, `"/"`, `"B"`) carrying `#task`, excluding the current line.                                                                                                              | Mirrors `^^` and Tasks' global filter; a `dependsOn` pointing at a non-task is meaningless to Tasks. Reuses the plugin's existing `getOpenObsidianTaskLines()` (frontmatter/fence aware). **Easily relaxable** if Bryan ever wants all checkboxes.                                                                                                                                                   |
| **Self-exclusion**         | The current bullet's line is never offered.                                                                                                                                                                                 | A task cannot depend on itself.                                                                                                                                                                                                                                                                                                                                                                      |
| **Already-a-dependency**   | Marked in the list; selecting it is a friendly no-op (no duplicate).                                                                                                                                                        | Predictable; avoids corrupting the list. (Future polish: toggle-to-remove.)                                                                                                                                                                                                                                                                                                                          |
| **Lenient source line**    | Stage 1 requires only a bullet; `dependsOn` is meaningful only on a `#task` line but we don't block.                                                                                                                        | Consistent with the existing picker, which operates on any bullet. Noted, not enforced.                                                                                                                                                                                                                                                                                                              |

## 3. Affected Repositories & Files

Two linked repositories. **No `bob-cli` (Rust) changes** — `bob-cli` only _deploys_ these artifacts via
`bob plugins sync` / `chezmoi apply`.

### A. `chezmoi` (config source of truth)

- `home/dot_config/bob/config.yml` — add the `dependsOn` property and document the new `local_task_id` value form.
  Deployed to `~/.config/bob/config.yml` via `chezmoi apply`.

### B. `bob-plugins` (Obsidian plugin source of truth) — all code changes in `plugins/bob-navigation-hotkeys/`

- `main.js` — config validation for the new value form, a thin task-enumeration layer, the target-identity resolver, the
  dependsOn merge writer, and the new value/block-ID modal stages.
- `styles.css` — task-row status pills, dependency / `+id` badges, and the block-ID preview (ported from
  `block-id-prompt`'s `bid-tlp-*` language into `bob-cnp-*` classes).
- `manifest.json` — bump the minor version (currently `1.2.0` → `1.3.0`; if the area/project badge plans land first,
  take the next available minor).
- `README.md` — update the plugin table entry for `bob-navigation-hotkeys`.

> The `^^` reference implementation lives in the separate `block-id-prompt` plugin. Per this repo's convention, helpers
> are **duplicated** into `bob-navigation-hotkeys` rather than imported across plugin boundaries — but note that
> `bob-navigation-hotkeys` _already_ contains most of the needed primitives, so duplication is minimal.

## 4. Technical Design

### 4.1 Config (chezmoi `home/dot_config/bob/config.yml`)

Document a third allowed `values` form and add the property:

```yaml
# `values` is the literal string `date` (YYYY-MM-DD), the literal string `local_task_id` (the identifier of an open
# #task chosen from the current file — its block ID), or a list of allowed scalar values.
properties:
  - name: scheduled
    values: date
  - name: p
    values: [1, 2]
  - name: dependsOn
    values: local_task_id
```

### 4.2 Config validation (`main.js`)

`normalizeBulletPropertyValues(name, values, options)` currently special-cases `"date"`, else requires a non-empty
scalar array. Add a sibling branch accepting `values === "local_task_id"` (return the literal), and update the error
message to `… must be "date", "local_task_id", or a non-empty scalar list`. `validateBulletPropertyConfig` delegates, so
no further change there.

### 4.3 New pure helpers (added near the existing task/bullet helpers; exposed on `module.exports.helpers`)

Reuse the plugin's existing primitives wherever possible — `getOpenObsidianTaskLines(lines)` (frontmatter/fence-aware,
`#task` + open-status filtered), `getTrailingBlockIdSpan(line)`, `OBSIDIAN_TASK_LINE_RE`, `findBulletPropertyField` /
`parseBulletPropertyFields`, `getBulletPropertyAppendIndex`, `formatBulletPropertyField`, and the block-ID/field
regexes. New small additions:

- `getOpenLocalTasks(content, { excludeLine })` →
  `[{ line, status, existingBlockId, existingIdField, displayText, rawLine }]`. Thin layer over
  `getOpenObsidianTaskLines`: for each returned index (skipping `excludeLine`), read the status (`OBSIDIAN_TASK_LINE_RE`
  group 1), the trailing block ID (`getTrailingBlockIdSpan`), the `[id::]` field value
  (`findBulletPropertyField(line, "id")`), and the cleaned label.
- `cleanTaskDisplayText(line)` → human-readable label (strip checkbox prefix, trailing block ID, **all** inline
  `[k:: v]` fields, Tasks emoji dates, the `#task` tag; collapse whitespace; fall back to `"(untitled task)"`). Ported
  from `block-id-prompt`'s `cleanTaskDisplayText` (more thorough than `parseProjectSourceTaskLine`, which keeps
  unrelated inline fields/emoji). Used for the row title and to seed the slug.
- `getTrailingBlockId(line)` → block-ID string or `null` (capture variant; `getTrailingBlockIdSpan` already gives the
  span — extract `^…` from it).
- `blockIdExistsInContent(content, id)` → boolean uniqueness scan (`(^|[ \t])\^<id>(?=$|[ \t\r\n])`), mirroring
  `block-id-prompt`'s `blockTokenMatches`.
- `suggestBlockIdFromTask(displayText, content)` → **deterministic** slug: lowercase, spaces→`-`, drop chars outside
  `[A-Za-z0-9-]`, collapse/trim hyphens, truncate (~32 chars); empty → `task`; ensure uniqueness vs `content` by
  appending `-2`, `-3`, …. No randomness — readable, stable, testable IDs.
- `appendBlockIdToLine(line, id)` → line with ` ^id` inserted before trailing whitespace (line-level analogue of
  `block-id-prompt`'s append edit).
- `resolveTargetTaskIdentity(line)` → `{ value, needsBlockIdPrompt, targetEdits }` implementing the decision tree:
  - `[id::]` present → `value = idField`; if no block ID, `targetEdits` appends ` ^idField`; never rewrite `[id::]`.
  - else block ID present → `value = blockId`; `targetEdits` inserts `[id:: blockId]`.
  - else → `needsBlockIdPrompt = true` (value/edits deferred to the block-ID stage).
- `upsertLocalTaskIdValue(line, name, id)` → dependsOn-aware **merge**: if `[name:: a, b]` exists, append `id` unless
  present (de-dupe) and re-serialize preserving order; else insert a fresh `[name:: id]` at
  `getBulletPropertyAppendIndex` (before the trailing block ID). Returns `{ line, changed, alreadyPresent }`. (Distinct
  from `upsertBulletProperty`, which _replaces_.)

### 4.4 Modal flow (`BulletPropertyPickerModal`) — three stages

Today the modal has `"properties"` → `"value"`. Add a `"blockid"` stage. The `FilteredPickerModal` contract is reused
throughout: `openItem` returning truthy closes the modal, falsy keeps it open (exactly how `properties → value` already
transitions).

**Stage 2 for `local_task_id` (branch inside `showValueStage`).** When `property.values === "local_task_id"`:

- Enumerate `getOpenLocalTasks(editor.getValue(), { excludeLine: cursor.line })`.
- Parse the current line's existing `dependsOn` IDs (`findBulletPropertyField(lineText, "dependsOn")`) to mark
  already-linked tasks.
- Header icon `"link"`; placeholder `"Filter open tasks"`; results label `"Open tasks"`; empty text
  `"No open tasks in this file"`; footer hints `↵ Link` / `esc Dismiss`.
- `renderItem` → `renderTaskValueItem`: a status pill (`[ ]` / `[/]` / `[B]`), the cleaned title with query highlighting
  (`appendHighlighted`), a `Line N` meta row, and a trailing badge: `✓ depends` when already linked, `↵ ^<id>` when the
  task already has an identifier, or `+ id` when one must be created.
- `openItem` → `chooseTaskDependency(item)`.

**`chooseTaskDependency(item)`:**

- Already a dependency → `Notice("Already depends on this task")`, return `false` (stay open).
- `resolveTargetTaskIdentity(taskLine)`:
  - `needsBlockIdPrompt` → `showBlockIdStage(task)`, return `false`.
  - else → apply `targetEdits` to the target line (if any) and write the dependency on the current line (§4.5); return
    `true` (close).

**Stage 3 (`showBlockIdStage(task)`):** set `stage = "blockid"`, store `pendingTask`, pre-fill the search input with
`suggestBlockIdFromTask(task.displayText, content)`, header icon `"hash"`, title `"New block ID"`, placeholder
`"Block ID — letters, numbers, hyphens"`, footer hints `↵ Create & link` / `esc Cancel`. Reuse the **existing
`getFilteredItems` override pattern** (the one that injects a synthetic typed-date item): when `stage === "blockid"`,
return a single synthetic **preview** item computed from `getRawQuery()` — showing the candidate ID, a
valid/invalid/duplicate state (check vs. alert icon + message), and `→ adds [id:: <id>] ^<id> to: <task title>`. Typing
re-renders the preview live. The preview item's `openItem` runs `confirmBlockId()` — so the existing Enter →
`openItemAtIndex(0)` path confirms it with no `handleKeydown` override needed (same as the date stage). `Esc` keeps its
default dismiss.

**`confirmBlockId()`:** read the typed ID; validate against the block-ID regex (`[A-Za-z0-9-]+`); check
`blockIdExistsInContent`. On failure, the preview shows the error and `openItem` returns `false` (stay). On success,
perform line-local edits in the current editor:

1. Re-read the target task line by number; verify it still matches the captured `rawLine` — if changed,
   `Notice("Task changed; dependency not added")` and abort. Otherwise replace it with `appendBlockIdToLine(line, id)`
   **and** insert `[id:: id]` (via the add-only `[id::]` path), yielding `… [id:: id] ^id`.
2. Write the dependency on the current line (§4.5) with `value = id`.

Then `Notice("Added ^<id> + linked dependency")` and close. Both edits replace a single full line addressed by line
number, so neither shifts the other's line — order-independent and safe.

### 4.5 Writing the dependency (`setLocalTaskDependency` on the plugin class)

A sibling to `setBulletPropertyValue`, but using `upsertLocalTaskIdValue` (merge) instead of `upsertBulletProperty`
(replace): re-read the current line, merge in the identifier, `replaceEditorLine`, restore the cursor with
`setEditorCursorSafely`, and emit a notice (`dependsOn → <id>` for a new dep, or an "already present" no-op notice).

### 4.6 Styling (`styles.css`)

Port the `block-id-prompt` task-row visual language into `bob-cnp-*` classes, reusing existing
`bob-cnp-row-text`/`-title` where possible: `.bob-cnp-status-pill` (+ `.is-active`, `.is-blocked`), `.bob-cnp-row-meta`,
`.bob-cnp-task-badge` (+ `.is-existing`, `.is-create`, `.is-linked`), and a small block-ID preview block
(valid/invalid/duplicate accent colors via the existing Obsidian theme variables, matching the date stage's aesthetic).

## 5. Interaction & Safety Notes

- **task-status-cycler stays inert.** Its normalizer only rewrites IDs that are Tasks-generated-shaped (`^[0-9a-z]{6}$`)
  _and_ inconsistent with the block ID. This feature writes human-readable slugs and keeps `[id::]` == block ID ==
  `dependsOn` value, so the normalizer finds nothing to change. (Document in a code comment.)
- **Add-only, never rewrite.** We only add an absent `[id::]` or an absent block ID; a pre-existing (possibly
  user-authored) `[id::]` is preserved and used as the dependency value, so the dependency always resolves in Tasks and
  we never corrupt intentional IDs.
- **Bare value is not a block token.** `[dependsOn:: foo]` contains no `^`, so uniqueness scans for a _new_ block ID
  won't false-positive on dependency values.
- **Mobile.** Config loading already returns the desktop-only notice when `fs` is unavailable; task enumeration uses the
  editor buffer (`getValue`), which works on mobile.

## 6. Verification

1. **Static:** `npm run validate` in `bob-plugins` (manifest checks + `node --check` syntax); `git diff --check`.
2. **Helper unit checks:** exercise the new pure helpers via Node's built-in `node:test`/`node:assert` against
   `module.exports.helpers` — open-task enumeration (frontmatter/fence exclusion, `#task` filter, self-exclusion);
   `resolveTargetTaskIdentity` across the four cases (no id / block-id-only / id-field-only / both); comma-list merge +
   de-dupe; deterministic slug + uniqueness suffixing; ` ^id` append; `[id::]` add-only; uniqueness scan. Run ad-hoc
   with `node --test`. (Committing a test file is optional, per the repo's no-test-runner philosophy.)
3. **Manual end-to-end in the Bob vault** (after `chezmoi apply` + `bob plugins sync -p bob-navigation-hotkeys` + plugin
   reload):
   - Cursor on a `#task` bullet → `Ctrl+Shift+P` → `dependsOn` appears → second stage lists the file's other open tasks.
   - Pick a task that **already has** `^id` but no `[id::]` → target gains `[id:: id]` (before `^id`); current line
     gains `[dependsOn:: id]`.
   - Pick a task with **no** id at all → block-ID stage pre-filled with a slug → `Enter` → target gains `[id:: id] ^id`;
     current line gains `[dependsOn:: id]`.
   - Pick a task that already has `[id:: x]` → current line gains `[dependsOn:: x]` (existing id respected); no rewrite.
   - Add a second dependency → value becomes `[dependsOn:: id1, id2]` (merged, de-duped).
   - Re-pick an already-linked task → "Already depends" notice, no duplicate.
   - Current task absent from the list (self-exclusion); tasks in frontmatter/code fences excluded; empty-state shown
     when no other open tasks exist.
   - Confirm Obsidian Tasks recognizes the dependency (e.g. a `tasks` query / the dependency hover), and that
     task-status-cycler does not rewrite the new values.

## 7. Rollout / Deploy

1. `chezmoi` — edit `home/dot_config/bob/config.yml`; `chezmoi apply` to write `~/.config/bob/config.yml`.
2. `bob-plugins` — edit `main.js` / `styles.css` / `manifest.json` / `README.md`; `npm run validate`.
3. Deploy: `bob plugins sync -p bob-navigation-hotkeys` (in a SASE workspace pass `-r "$PWD"`; preview with `--dry-run`;
   mind the dirty-vault `--force` caveat), then reload Obsidian / re-enable the plugin.

## 8. Out of Scope

- No `bob-cli` (Rust) changes; it only deploys these files.
- No changes to Obsidian Tasks or to task-status-cycler's normalizer.
- Toggle-to-remove an existing dependency from the picker (possible future polish).
- Cross-file dependencies — scoped to **local** (same-file) tasks, per `local_task_id`.
