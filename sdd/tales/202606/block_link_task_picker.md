---
create_time: 2026-06-24 07:52:49
status: wip
prompt: sdd/prompts/202606/block_link_task_picker.md
---
# Plan: Block‑link task picker (`^^` → pick an open task → complete the link)

## Repository

This feature lives in the **`bob-plugins`** linked repo (source‑of‑truth monorepo for Bryan's Obsidian plugins; deployed
to `~/bob/` via `bob plugins sync`). All file paths below are relative to the `bob-plugins` repo root. To read/edit
`bob-plugins` from a numbered workspace, open it with `sase workspace open -p bob-plugins -r "<reason>" <workspace_num>`
and use the printed path.

## Goal (product)

While typing a wiki **block link**, the moment the user types a _second_ caret immediately after the first — producing
`[[<target>^^]]` (or `[[<target>#^^]]`) with **nothing** after the carets — pop a polished menu of every **open task**
in the note that `<target>` points at. Picking a task **completes the block link** so it points at that task's block.

- If the chosen task already has a block id (a trailing `^id`), the link is completed immediately with that id.
- If the chosen task has **no** block id, prompt the user for one; the new id is appended to the task line **in the
  target note** and then used to complete the link.

Example: as the user types the last `^` in `[[foobar^^]]`, the menu opens listing the open tasks in `foobar.md`;
choosing one yields e.g. `[[foobar^plan-api]]` (creating `^plan-api` on that task line in `foobar.md` if it didn't
exist).

The feature must be **intuitive** (matches how the existing `^`/`^^` markers already behave), **reliable** (no races, no
lost edits, no missed triggers), and **beautiful** (reuses the command‑palette‑class picker idiom already proven in this
repo).

## Where this belongs: extend the `block-id-prompt` plugin (not a new plugin)

`block-id-prompt` already:

- **Owns the `^^` marker convention** ("Prompt for a custom block ID when a wiki block link uses the `^^` marker") and
  is the **only** component that scans editor changes for caret‑markers inside wiki links. It already handles two
  adjacent behaviors on the very same keystrokes:
  - the **file‑link jump**: typing `^` right after `[[foobar]]` rewrites it to `[[foobar^]]` with the cursor placed
    inside, after the caret (`parseTrailingCaretCompletionMarker` → `kind: "file-link-jump"`); and
  - the **block‑id rename**: `[[foobar#^^existingId]]` (a doubled caret _with_ an existing id) opens the rename prompt
    (`parseInlineMarkerLink`).
- Implements exactly the **hard part of step 3** — assigning/renaming a block id safely in a _possibly different_ note:
  candidate discovery via the metadata cache, duplicate‑id detection (`blockTokenMatches`), stale‑content verification,
  and read/verify/modify of the target file (`renameDestinationBlock`, `submitDirectBlockAdd`,
  `readDestinationForValidation`).

Putting this feature anywhere else means a **second** `EditorView.updateListener` reinterpreting the same mid‑type
`[[…^…]]` keystrokes, which would **race** with `block-id-prompt`'s own auto‑edits (the jump rewrite, the rename).
Keeping it in one plugin gives **one coherent decision tree per keystroke**: "is this an empty `^^` (→ task picker), a
`#^^id` (→ rename), or a `]]^` (→ jump)?"

The empty‑`^^` case is currently **inert** in `block-id-prompt` (`parseInlineMarkerLink` requires a non‑empty id after
`#^^`, and the bare `^^` form resolves to nothing), so this is a clean, collision‑free hook.

**Alternative considered — a new standalone plugin (`block-task-link`):** rejected. It would duplicate the wiki‑link
parsing + the entire block‑id safety machinery _and_ introduce the two‑listener race above. The only thing it would buy
is a separate version stream, which is not worth the coordination hazard.

Following the repo's established ethos (see `bob-navigation-hotkeys`, which _duplicates_ the small Pomodoro constants on
purpose "so this plugin does not reach into another plugin's non‑public module internals"), the **picker UI, the
open‑task scanner, and the picker CSS are duplicated into `block-id-prompt`** rather than imported across plugins.

Version: bump `plugins/block-id-prompt/manifest.json` `1.0.0 → 1.1.0`; broaden its `description`; update the `README.md`
plugin row + layout note (this plugin will now also ship a `styles.css`).

## Trigger detection

Fire the picker when **all** hold:

1. The active Markdown editor has a **single** cursor/selection (reuse `hasSingleCursor`).
2. The cursor's line contains a wiki link `[[…]]` whose destination ends in `^^` with an **empty** id — i.e.
   `[[<target>^^]]` (bare caret form) or `[[<target>#^^]]` (canonical form), with optional alias suffix `|Alias`.
   "Empty" = nothing between the `^^` and the closing `]]` / `|`.
3. `<target>` resolves to an existing `.md` file via `metadataCache.getFirstLinkpathDest` (reuse
   `resolveReferenceDestination`). An empty target (`[[^^]]`, a self‑link) resolves to the current note.
4. The cursor is **not** inside a fenced code block (reuse `lineIsInsideCodeFence`).

This trigger is the natural product of the existing jump: `[[foobar]]` `+^` → `[[foobar^]]` (cursor inside) `+^` →
`[[foobar^^]]`. It also works when the user types `^^` or `#^^` directly.

**Disjointness from existing markers** (so nothing collides): the rename marker requires a **non‑empty** id after `#^^`;
the jump requires a caret **after** `]]`. The new trigger is the empty‑id‑inside case, which today does nothing. Add a
new parser (e.g. `parseTrailingTaskPickerMarker`) and slot it into the scan so the empty‑`^^` case is recognized
**before** falling through to the existing parsers.

### Reliability: don't get swallowed by the self‑edit suppression

`block-id-prompt` suppresses scanning for `EDIT_SUPPRESS_MS` (250 ms) after any of its own programmatic edits
(`suppressEditorScans`), including the auto‑jump. Because the second caret is frequently typed **immediately after** the
jump rewrite, a naive implementation would drop that keystroke's scan and the menu would not open until the user typed
again.

Fix: restructure the scan path so a doc change **always schedules a scan**, but inside `inspectActiveEditor` only the
**jump** and **rename** behaviors stay gated on "not currently suppressed"; the **task‑picker** trigger is evaluated
regardless of the suppression window. This is safe because the plugin **never** programmatically produces an empty‑`^^`
link, so the picker trigger can never self‑retrigger from the plugin's own edits.

While the picker (or the follow‑up id prompt) is open, set a guard (reuse/extend the existing `promptOpen` flag, or add
a parallel `menuOpen`) so further scans are ignored and the menu can't stack.

## What counts as an "open task" (menu contents)

Mirror the **canonical** definition already used by `bob-navigation-hotkeys` (`isOpenObsidianTaskLine` /
`getOpenObsidianTaskLines`):

- A checkbox list item (`OBSIDIAN_TASK_LINE_RE`: indentation, blockquote prefixes, ordered or unordered markers all
  allowed) …
- … whose status symbol ∈ `{" ", "/", "B"}` (`OPEN_OBSIDIAN_TASK_STATUSES` — todo, in‑progress, blocked; **excludes**
  done `x` and cancelled `-`) …
- … **and** whose body carries a standalone `#task` tag (the vault's Tasks "global filter", `PROJECT_TASK_TAG_RE`).
- Scan the **whole file**, skipping leading frontmatter and fenced code blocks with the same state machine
  `getOpenObsidianTaskLines` uses (so task‑shaped lines inside YAML, examples, and `tasks`/`dataview` query blocks are
  ignored).

Rationale: `#task` is the vault's Tasks global filter, so this matches what every other piece of Bryan's tooling calls a
"task," and "open" means the same three statuses used by the project counter (`bob-project-tasks`) and the task
navigator. Duplicate this small scanner into `block-id-prompt` (don't cross‑import) per repo ethos.

**Alternative considered — all open checkboxes regardless of `#task`:** rejected; it would surface ordinary checklist
items that aren't real tasks, contradicting the vault's task definition.

Each menu item carries: `line` index, raw line text, an existing block id (if the line already ends with `^id`, via
`TRAILING_BLOCK_ID_RE`), the status symbol, and a **cleaned display string** for the row (strip the leading list
marker + checkbox, the `#task` tag, any trailing `^id`, and Tasks inline metadata like
`[completion:: …]`/`[id:: …]`/emoji dates so the row reads as the task's actual description).

## The picker UI (beautiful + consistent)

Add a `TaskLinkPickerModal` modeled on the proven `FilteredPickerModal` idiom in `bob-navigation-hotkeys`:

- **Header**: accent icon (e.g. `list-checks`), title "Link to task", subtitle "`<n>` open tasks in `<basename>`" (live
  "Showing X of Y" when filtering).
- **Search input** with a search icon; substring/fuzzy filter over the cleaned task text.
- **Results list** (`role=listbox`, rows `role=option`): each row shows a small **status pill** (`[ ]` / `[/]` / `[B]`),
  the highlighted task text, and a right‑aligned **badge**:
  - `↵ link` (muted) plus the existing id (e.g. `^plan-api`) when the task already has a block id; or
  - `+ id` (accent) when an id will be created on selection.
- **Keyboard**: `↑/↓` and `^N/^P` to move, `Enter`/click to choose, `Esc` to dismiss; footer key hints. Selected row
  scrolls into view.
- **Empty state**: when the target has zero open tasks, show a friendly empty panel ("No open tasks in `<basename>`")
  instead of doing nothing, so the trigger always feels responsive.
- **Safety**: render with `createEl`/`appendText`/`appendHighlighted` only — **never** `innerHTML` (task text is
  untrusted).

**Self‑contained styling:** add a **new** `plugins/block-id-prompt/styles.css` containing a ported subset of the
`bob-cnp-*` rules under a **fresh namespace** (e.g. `bid-tlp-*`). This keeps `block-id-prompt` self‑styled — it must not
depend on `bob-navigation-hotkeys` being enabled — and avoids global class collisions. Duplicate the tiny `applyIcon` +
`appendHighlighted` helpers into `block-id-prompt` as well. Keep `isDesktopOnly: false`; the picker is touch‑friendly
(click handlers, no hover‑only affordances).

**Alternative considered — an inline `EditorSuggest` popup** (like Obsidian's native `[[` and `#^` suggesters):
rejected. The repo has no `EditorSuggest` precedent (every picker here is a `Modal`), an `EditorSuggest` triggering on
`^` risks fighting Obsidian's native block suggester on the `#^` path, and the secondary "enter a block id" step is a
modal regardless. The `Modal` picker is the consistent, reliable, already‑beautiful choice and reads naturally as "a
nice menu."

## Selecting a task → completing the link

Two cases.

**A) Task already has a block id (`^id`).**

1. Re‑read the target note and **re‑verify** the chosen task line is unchanged and still ends with that `^id` (guard
   against edits made while the menu was open).
2. Rewrite the **source** link's empty `^^` marker to `^<id>`, **preserving the user's caret style** (bare `^^` → `^id`;
   `#^^` → `#^id`) and any alias suffix → e.g. `[[foobar^plan-api]]` or `[[foobar^plan-api|Alias]]`. Place the cursor
   just after the closing `]]`. Suppress the self‑rescan during the rewrite.

**B) Task has no block id.**

1. Open the block‑id prompt by **reusing the existing `BlockIdPromptModal` shell** seeded with a **new source kind**
   `link-task-complete`, and pre‑set `previewText` to the chosen task's text (so the modal's preview pane shows the task
   being linked). Input starts empty; validate against `BLOCK_ID_RE` (letters, numbers, hyphens).
2. On submit, route through a new `submitLinkTaskBlockId(source, newId)` branch in `submitBlockId` that:
   - Re‑reads the target note; verifies the chosen task line is **still present and unchanged**; verifies `newId` is
     **not** already a block id in the target (`blockTokenMatches`).
   - **Appends** ` ^<id>` to that task line in the **target note** — via the live editor when the target _is_ the active
     note (self‑link), otherwise via `vault.process`/`vault.modify`, mirroring `renameDestinationBlock`'s
     active‑vs‑other handling and the trailing‑whitespace rules used by `submitDirectBlockAdd`.
   - Rewrites the **source** link to `^<id>` (same logic as case A).
   - Emits a success `Notice`; every failure path emits a guard `Notice` mirroring existing copy and leaves the document
     safe.

In both cases the doubled `^^` collapses to a single `^<id>`, so the link the user ends up with is the ordinary block
link they were reaching for.

## Cancel / dismiss behavior

If the user dismisses the picker (or the follow‑up id prompt) **without choosing**, revert the **triggering second
caret**, returning the link to `[[<target>^]]` with the cursor right after the caret — i.e. the post‑jump state. This
makes the second `^` behave like a transient _trigger key_ rather than leaving an inert `[[<target>^^]]` that would
re‑fire the menu on the next keystroke. Suppress the rescan during the revert. (If the user _did_ choose, no revert; the
link is completed as above.)

## Edge cases & safety

- **Self‑link** `[[thisNote^^]]` (or `[[^^]]`): read tasks from the **live editor buffer** and write the appended id via
  the editor (mirror the source‑equals‑destination branch already in `renameDestinationBlock`).
- **Target changed between menu‑open and selection**: re‑verify before writing; abort with a `Notice` if the chosen line
  no longer matches.
- **Duplicate id**, **unreadable/unresolvable target**, **multiple cursors**, **inside code fence**: guarded with
  `Notice`s / no‑op, reusing existing helpers.
- **Aliased links** `[[foobar^^|Alias]]`: split the alias suffix (`splitWikiLinkBody`) and preserve it in the
  completion.
- **Zero open tasks**: show the picker's empty state; `Esc` reverts the caret.
- Keep behavior independent of whether `bob-navigation-hotkeys`/`bob-project-tasks` are enabled.

## Files to change

- `plugins/block-id-prompt/main.js`
  - New constants + scanner: `OBSIDIAN_TASK_LINE_RE`, `OPEN_OBSIDIAN_TASK_STATUSES`, `#task` matcher, frontmatter/fence
    skipping, `getOpenTasksInContent()` returning rich task items (mirrors `getOpenObsidianTaskLines`).
  - New marker parser `parseTrailingTaskPickerMarker` + wiring into the scan path, plus the suppression‑window
    restructuring described above.
  - `TaskLinkPickerModal` + duplicated `applyIcon` / `appendHighlighted` helpers.
  - New source kind `link-task-complete` and `submitLinkTaskBlockId` branch; the link‑completion rewrite helper
    (caret‑style‑preserving); the cancel‑revert helper.
- `plugins/block-id-prompt/styles.css` — **new**; namespaced (`bid-tlp-*`) picker styles (ported subset of `bob-cnp-*`).
- `plugins/block-id-prompt/manifest.json` — `version → 1.1.0`; broadened `description`.
- `README.md` — update the `block-id-prompt` row (version + description) and the layout/styles note to reflect that
  `block-id-prompt` now ships a `styles.css`.

## Validation & deployment

- `npm run validate` (manifest fields + `node --check` on `main.js`). The new `styles.css` is allowed (only
  `manifest.json`/`main.js`/`styles.css` are synced).
- Deploy to the vault from the `bob-plugins` workspace: `bob plugins sync -p block-id-prompt -r "$PWD"` (the `-r "$PWD"`
  reason flag is required when syncing from a SASE workspace).

## Manual test matrix (in‑vault)

Create a `foobar.md` containing a mix of: open `#task` lines (statuses `[ ]`, `[/]`, `[B]`), a done `[x]` `#task`, a
cancelled `[-]` `#task`, a plain checklist item **without** `#task`, a task with an existing `^id`, and a fenced code
block containing a task‑shaped line. From another note:

1. Type `[[foobar]]`, then `^` → expect `[[foobar^]]` (existing jump), then `^` → **menu opens** listing **only** the
   open `#task` lines (the done/cancelled/untagged/fenced lines are absent).
2. Pick a task **with** an id → link completes to `[[foobar^<existingId>]]`; `foobar.md` unchanged.
3. Pick a task **without** an id → id prompt appears showing the task text; enter `my-id` → `^my-id` is appended to that
   task line in `foobar.md` and the link completes to `[[foobar^my-id]]`.
4. `Esc` from the menu → link reverts to `[[foobar^]]`, cursor after the caret.
5. **Fast‑typing**: type the two carets in quick succession right after the jump → menu still opens (suppression‑window
   fix).
6. **Self‑link**: from `foobar.md` itself, `[[foobar^^]]` lists its own open tasks and edits in place.
7. **Alias**: `[[foobar^^|Title]]` completes to `[[foobar^<id>|Title]]`.
8. `npm run validate` passes.
