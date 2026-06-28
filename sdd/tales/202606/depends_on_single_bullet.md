---
create_time: 2026-06-28 14:45:26
status: wip
prompt: sdd/prompts/202606/depends_on_single_bullet.md
---
# Plan: Single, Sub-projects-styled `**DEPENDS ON:**` navigation bullet

## Repository

This feature lives entirely in the **`bob-plugins`** linked repo, under `plugins/bob-navigation-hotkeys/`. It does
**not** touch the `bob-cli` Rust code or edit the vault plugin copy directly. After the code is implemented and
validated in the `bob-plugins` checkout, deploy with `bob plugins sync` (scoped to `bob-navigation-hotkeys`) as the
linked-repo instructions require.

Baseline: `bob-navigation-hotkeys` `1.7.0`, which writes **one managed child bullet per dependency**:

```
  - **DEPENDS ON:** [[#^demo-video-research]]
  - **DEPENDS ON:** [[#^user-note]]
  - **DEPENDS ON:** [[#^fast-revive-neighbors]]
```

## Goal

Collapse those per-dependency bullets into **one** bullet, styled like the vault's existing `**Sub-projects:**` bullet —
an emoji prefix, a bold label, and the targets joined by `•` separators on a single (wrapping) line:

```
  - 🔗 **DEPENDS ON:** [[#^demo-video-research]] • [[#^user-note]] • [[#^fast-revive-neighbors]]
```

For reference, the Sub-projects bullet this mirrors is authored as:

```
- 🧩 **Sub-projects:** [[sase_blog]] • [[sase_config]] • [[sase_dyn_agent_fam]] • ~~[[sase_anti_gravity]]~~ ✅ • …
```

The result must be **intuitive** (reads at a glance, one tidy line per task), **reliable** (no lost or duplicated links,
safe migration of existing notes, no regression to the dependency picker), and **beautiful** (visually consistent with
Sub-projects in Obsidian's live preview).

## Why this is low-risk (key architectural finding)

The dependency picker's correctness is driven by the canonical **`[dependsOn:: …]` inline field** on the task, not by
the navigation bullets:

- `showLocalTaskValueStage` builds the "already linked" set from `parseLocalTaskIdList(getCurrentPropertyValue(...))` —
  i.e. the `[dependsOn:: …]` field — and passes it to `createBulletPropertyLocalTaskItems` as `dependencyValues`.
- `alreadyLinked`, the add/remove staging, and dedupe all key off that field.

The `**DEPENDS ON:**` child bullets are a **pure derived, human-navigation layer**. Changing them from "N lines" to "1
line with N links" therefore cannot break picker logic, the Tasks/Dataview dependency model, or the `task-status-cycler`
normalizer (which rewrites the `[dependsOn:: …]` field, never the nav bullet). This is what makes the change safe to do
as a format/rendering refactor of the navigation layer alone.

## Terminology (unchanged from 1.7.0)

- **Dependency value** — the bare block id stored in `[dependsOn:: …]` (prefers an existing `[id:: …]`, else the task's
  trailing `^block-id`). Comma-separated; canonical source of truth.
- **Navigation block id** (`linkBlockId`) — the trailing `^block-id` a `[[#^…]]` nav link points at. May differ from the
  dependency value when a task has `[id:: x]` but a different trailing `^block`.

The nav layer keys exclusively off `linkBlockId`. The dependency value vs. nav block id can diverge; the design never
assumes they are equal.

## Product behavior

### Rendering

A task with one or more managed dependencies renders exactly **one** child bullet:

```
  - 🔗 **DEPENDS ON:** [[#^id-a]] • [[#^id-b]] • [[#^id-c]]
```

- **Emoji:** `🔗` (link/navigate). Firm recommendation — it reads as "jump to these prerequisites" and is visually
  distinct from Sub-projects' `🧩`. It is a single one-line constant; alternatives (`⛓️`, `🚧`, `⬆️`) are trivial to
  swap. The plan assumes `🔗`.
- **Separator:** `•` — the same space-padded U+2022 bullet Sub-projects uses, so the two bullets look like siblings.
- **Label:** `DEPENDS ON` (unchanged), still bold via `**…**`.
- **Order:** preserve the existing on-screen link order and **append** newly added links at the end (see "Ordering"
  below). In the common case this already matches `[dependsOn:: …]` order.
- **Zero dependencies:** no bullet at all (the single bullet is deleted when the last link is removed).

No `styles.css` change is needed: this is plain markdown the plugin writes; Obsidian renders the emoji, bold label, and
links. "Beautiful" is achieved by the markdown format itself, exactly as Sub-projects is.

### Editing through the picker (no UX change)

The `<Tab>` batch add/remove flow, the sequential block-ID prompts (1.7.0), and the single-`<Enter>` flow are all
**unchanged from the user's perspective**. The only difference is the shape of the bullet that gets written: every
commit now produces/updates the single consolidated bullet instead of separate lines.

### Migration of existing notes

Two complementary paths, so the rollout is clean and never forces re-editing:

1. **Lazy (automatic):** any task touched through the picker has its nav bullets rewritten to the consolidated form on
   commit. Pre-existing legacy single-link bullets under that task are collapsed into the one bullet and gain the emoji.
2. **One-shot command (recommended):** a new command, e.g. **"Consolidate DEPENDS ON navigation links (current note)"**,
   scans every task in the active note and rewrites each task's managed dependency bullets into a single consolidated
   bullet — without changing any `[dependsOn:: …]` field. This lets the user convert a whole note (or the visible note
   in the screenshot) in one action. It is idempotent: running it on an already-consolidated note is a no-op.

Migration preserves link order and dedupes; it never invents, drops, or reorders dependencies beyond de-duplication.

## Technical design

### 1. Constants and format

- Add `DEPENDENCY_NAVIGATION_EMOJI = "🔗"` and `DEPENDENCY_NAVIGATION_SEPARATOR = " • "` (the latter shared with / equal
  to the Sub-projects separator).
- Rework `formatDependencyNavigationBullet` to accept an **ordered list of block ids** (keep tolerant of a single string
  for back-compat) and render: `${indent}- ${EMOJI} **DEPENDS ON:** [[#^a]] • [[#^b]] • …`. An empty list yields no
  bullet (callers handle deletion rather than writing an empty bullet).
- Keep `formatDependencyNavigationBulletFromDetails` for in-place rewrites, updated to emit the emoji + joined links
  from parsed details, preserving the original indent/marker.

### 2. Parsing — one bullet, many links

- Replace `DEPENDENCY_NAVIGATION_BULLET_RE` with a regex that matches a single managed bullet containing **one or more**
  `•`-joined `[[#^id]]` links, with the emoji prefix **optional** (so legacy emoji-less bullets still parse) and
  recognizing the current `DEPENDS ON` label plus legacy labels (`DEPENDENCIES`). Capture `indent`, `marker`, `label`,
  and the raw link span.
- `parseDependencyNavigationBulletDetails(line)` returns
  `{ indent, marker, label, blockIds: string[], isLegacy, hasEmoji }`, extracting **all** block ids from the link span
  (tolerant of separator spacing, and tolerant of `~~…~~`/`✅` decoration so dedupe/removal still work if a target was
  manually struck through). `isLegacy` (legacy label) and `hasEmoji === false` both signal "not yet canonical".
- Provide `getDependencyNavigationBlockIds(line) -> string[]` (the new primary accessor). Keep
  `parseDependencyNavigationBullet(line)` returning the **first** block id (or null) for any narrow internal use, but
  migrate call sites to the array accessor.

### 3. Collect + plan as a single-line sync

Replace the per-block insert/remove/normalize helpers with a **collect → compute → sync** model:

- `collectDependencyNavigationBullets(content, parentLine)` — scan the parent's child block
  (`findCurrentBulletChildBlock`) and return
  `{ lineIndices: number[], blockIds: string[] (ordered union, deduped), indent, marker, anyLegacy }`. Multiple managed
  bullets (legacy or mixed) are unioned in document order.
- `computeFinalDependencyLinkOrder(existingIds, addIds, removeIds) -> string[]` — start from `existingIds`, drop
  `removeIds`, append `addIds` not already present, dedupe preserving first occurrence (**Approach B: preserve existing
  order, append new**). This sidesteps the dependency-value-vs-nav-block-id divergence entirely, since it operates
  purely on `linkBlockId`s and never needs to map nav ids back to `[dependsOn::]` values.
- `planDependencyNavigationBulletSync(content, parentLine, finalBlockIds) -> plan` — a **pure** plan that reconciles the
  child block to exactly the `finalBlockIds`:
  - `guard` — parent out of range / not a bullet.
  - `noop` — already exactly one canonical bullet with these ids in this order, emoji present.
  - `rewrite` — replace the **first** managed bullet line with the regenerated single bullet (reusing its indent +
    marker) and **delete** any other managed bullet lines (this is where legacy multi-bullets collapse + gain the
    emoji).
  - `insert` — no managed bullet exists and `finalBlockIds` is non-empty: insert one fresh bullet at the child block
    start (matching today's position), indent via `getDependencyChildIndent`, `- ` marker.
  - `delete` — `finalBlockIds` is empty: delete every managed bullet line.
  - The plan exposes the concrete edits (`replaceLine`/`lineText`, `deleteLines: number[]`, `insertLine`) so the applier
    can run them deterministically against one content snapshot (apply deletes high-index-first, then the single
    replace/insert — no re-reading needed, no line-shift hazards).

This collect→compute→sync trio subsumes `planDependencyNavigationBulletInsertion`,
`planDependencyNavigationBulletRemoval`, and `planDependencyNavigationLabelNormalizations`: consolidation, emoji
upgrade, and legacy-label normalization all fall out of "regenerate the one canonical bullet."

### 4. Wire both commit paths through the sync

- **Batch executor** (`executeDependencyBatch` → `reconcileDependencyNavigationBullets`): after the `[dependsOn:: …]`
  list is rewritten, collect existing nav ids, compute the final order from `additions[].linkBlockId` /
  `removals[].linkBlockId`, build the sync plan against fresh editor content, and apply it. Report counts (added/removed
  links, and whether bullets were consolidated) in the existing notice.
- **Single-task path** (`setLocalTaskDependency`): replace the lone `planDependencyNavigationBulletInsertion` call with
  `collect` + `computeFinalDependencyLinkOrder(existingIds, [linkBlockId], [])` + `sync`, so single adds also
  produce/upgrade the one consolidated bullet (and collapse any stragglers under that task).
- Remove `normalizeDependencyNavigationLabels` call sites (now handled by sync), or keep the function as a thin wrapper
  over sync for the no-change-but-normalize case.

### 5. New consolidation command

Register a command (near `set-bullet-property`):

- id `consolidate-dependency-navigation-links`, name "Consolidate DEPENDS ON navigation links (current note)",
  `editorCallback`.
- Handler walks the note's task lines, and for each task with managed dependency bullets runs collect → sync (with the
  existing ids as the final order, i.e. no add/remove — pure reformat). Apply per-task edits bottom-up so earlier line
  indices stay valid. Show a summary notice (`Consolidated N task(s)` / `Nothing to consolidate`).
- Idempotent and safe to run repeatedly; touches only managed bullets, never `[dependsOn:: …]` fields or unrelated child
  bullets.

### 6. Notices, exports, metadata, docs

- Update `buildMultiDependencyNotice` / `buildLocalTaskDependencyNotice` wording so the navigation summary fits the
  single-bullet reality ("added/removed N link(s)", and "consolidated" when legacy bullets were collapsed).
- Update `module.exports.helpers`: add `DEPENDENCY_NAVIGATION_EMOJI`, `DEPENDENCY_NAVIGATION_SEPARATOR`,
  `getDependencyNavigationBlockIds`, `collectDependencyNavigationBullets`, `computeFinalDependencyLinkOrder`,
  `planDependencyNavigationBulletSync`; remove or alias the retired per-block planners.
- Bump `plugins/bob-navigation-hotkeys/manifest.json` `1.7.0` → `1.8.0`.
- Update the `bob-navigation-hotkeys` row in `README.md` to describe the single consolidated `🔗 **DEPENDS ON:**` bullet
  (Sub-projects style) and the new consolidation command.

## Scope decision: completion styling (strikethrough + ✅) is a deliberate non-goal for v1

Sub-projects shows completed entries as `~~[[…]]~~ ✅`. Applying the same to _done_ dependency tasks is attractive but
**out of scope for v1**, on purpose:

- It requires resolving each `^id` to its task line, reading checkbox state, and — critically — **re-rendering the
  bullet whenever any dependency task's status changes**, which needs editor/metadata-change hooks the plugin does not
  currently have.
- A one-time, non-live strikethrough would go stale and mislead — _less_ reliable, contradicting the goal.

To stay forward-compatible, the parser tolerates `~~…~~`/`✅` decoration when extracting block ids, so a future version
can layer completion styling on without breaking dedupe/removal. v1's canonical output is plain links. (Caveat to note
in the plan: because a change to any one link rewrites the whole bullet, manually-added decoration on sibling links
isn't preserved across edits — acceptable for v1.)

## Edge cases

- **Mixed legacy + already-consolidated bullets** under one task → unioned in order, collapsed to a single canonical
  bullet.
- **Remove last dependency** → bullet line deleted entirely (no empty `🔗 **DEPENDS ON:**`).
- **Duplicate ids** (across legacy bullets or repeated) → deduped, first occurrence wins.
- **Dependency value ≠ nav block id** → handled; ordering/sync key off `linkBlockId` only.
- **Indent/marker** → reuse the first existing managed bullet's indent + marker; fresh inserts use
  `getDependencyChildIndent` + `- `.
- **Legacy `DEPENDENCIES` label** and **emoji-less bullets** → parsed and upgraded to canonical on next sync.
- **Line-shift safety** → pure plan applied against one snapshot, deletes high-index-first then a single replace/insert.
- **Non-managed sibling bullets** and **other tasks' bullets** → untouched (scoped to the parent's child block +
  managed-bullet regex).
- **Other plugins** → unaffected; `task-status-cycler` operates on the `[dependsOn:: …]` field, not the nav bullet.
- **Stale target / changed cursor during batch prompts** → unchanged from 1.7.0 (skip-and-report / abort-before-write).

## Validation

Run from the `bob-plugins` checkout:

- `node --check plugins/bob-navigation-hotkeys/main.js`
- Focused Node checks for the new/changed pure helpers (mirroring the repo's `module.exports.helpers` convention):
  - `formatDependencyNavigationBullet` renders single / multi / (empty → caller-handled) correctly with emoji + `•`
    joins.
  - `parseDependencyNavigationBulletDetails` parses: new multi-link bullet, legacy single-link (no emoji), legacy
    `DEPENDENCIES` label, and a decorated `~~[[#^id]]~~ ✅` link (ids still extracted).
  - `collectDependencyNavigationBullets` returns the ordered, deduped union across multiple legacy bullets with line
    indices.
  - `computeFinalDependencyLinkOrder` appends new, drops removed, dedupes, preserves order.
  - `planDependencyNavigationBulletSync` yields `noop` / `rewrite` (collapse N→1 + emoji) / `insert` / `delete` /
    `guard` as specified.
- In-memory modal/editor checks (throwaway harness, since the repo ships no committed test suite):
  - Batch add of several deps writes exactly one consolidated bullet.
  - Committing under a task that already has 3 legacy single-link bullets collapses them into one (with emoji).
  - Removing one dep shrinks the single bullet; removing the last dep deletes it.
  - Single-`<Enter>` add produces/upgrades the single bullet.
  - Mixed add + remove in one batch yields the correct final single bullet.
  - The new consolidation command reformats a note's tasks idempotently and leaves `[dependsOn:: …]` untouched.
- `npm run validate`
- `git diff --check`
- `bob plugins sync -p bob-navigation-hotkeys -r "$PWD" --dry-run` (expect only `manifest.json`, `main.js`; `styles.css`
  only if it changed — it should not for this feature), then the real scoped
  `bob plugins sync -p bob-navigation-hotkeys -r "$PWD"`.

## Out of scope

- Live completion styling (strikethrough + ✅) of done dependencies — future enhancement (see scope decision).
- Any change to the `[dependsOn:: …]` field semantics, the Tasks/Dataview model, or `task-status-cycler`.
- Vault-wide batch migration across many notes in one command (the per-note command + lazy migration cover the need).
