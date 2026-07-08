---
create_time: 2026-07-08 12:38:02
status: wip
prompt: sdd/prompts/202607/option_bracket_transcluded_task_cycle.md
---
# Plan: Replace `@` Transcluded-Task Keymap with Counted `<option+[>` / `<option+]>` Cycling

## Goal

Remove the `@` Vim-normal keymap added yesterday (source-task open/in-progress toggle on transcluded block links) and
instead teach the **existing** `<option+]>` (cycle forward) and `<option+[>` (cycle backward) task-status keymaps to
operate on transcluded block links to Obsidian tasks. These keymaps must also honor an explicit Vim count: `N<option+]>`
cycles the task on the current line plus the next `N` lines (e.g. `2<option+[>` cycles the current task's status and the
Obsidian tasks on the next 2 lines).

Net effect: one consistent pair of keymaps cycles **any** task line — a direct checkbox task or a transcluded task block
link — forward/backward through the full status cycle, single-line by default and multi-line with a count. The `@`
keymap goes away entirely.

## Context Reviewed

- Read Obsidian long-term memory via the memory-read skill.
- Read yesterday's two SDD tales that introduced/repaired the `@` keymap:
  - `counted_transclusion_keymaps` (added counted `!` and `@`).
  - `fix_counted_transclusion_keymaps` (fixed the count read to use `inputState.keyBuffer`).
- Inspected the `bob-plugins` source plugin `plugins/task-status-cycler/main.js`, which owns both the `@` listener and
  the `cycle-task-status-forward` / `-backward` commands.
- Confirmed the live key bindings from the vault `hotkeys.json`:
  - `task-status-cycler:cycle-task-status-forward` = `Alt`+`]`
  - `task-status-cycler:cycle-task-status-backward` = `Alt`+`[`
- Confirmed the vault `obsidian_vimrc.md` has **no** `@` mapping and **no** bracket mappings tied to these commands, so
  `@` is a pure capture-phase listener and `<option+[>`/`<option+]>` are pure `hotkeys.json` bindings. No vimrc or
  hotkeys.json edits are required by this change.
- Verified the plugin README/manifest only carry a generic one-line description ("Cycle the active task line through
  configured Tasks statuses"), so no doc changes are needed.
- No `bob-cli` subcommands or options are involved, so `memory/cli_rules.md` is not required.

### Key facts about the current code (all in `plugins/task-status-cycler/main.js`)

- Status cycle order: `FIXED_SYMBOLS = [" ", "/", "B", "x", "-"]` (open, in-progress, blocked, done, cancelled).
  `getAdjacentSymbol(symbol, direction)` steps through this ring; `direction` is `+1` forward (`]`), `-1` backward
  (`[`).
- `handleCycleCommand(checking, editor, view, direction)` backs both cycle commands. Today it: (1) cycles the active
  line's checkbox via `getActiveTaskStatus` + `getAdjacentSymbol` + `setActiveCheckboxStatus`, else (2) falls back to
  plain-bullet formatting (`getActivePlainBulletFormatToggle` / `toggleActivePlainBulletFormat`). It has **no**
  transcluded-link handling and **no** count handling (Obsidian commands do not receive Vim counts).
- The `@` keymap is a capture-phase `keydown` listener registered by `registerTranscludedTaskStartInputListeners()`
  (called in `onload`), routed through `handleTranscludedTaskStartPhysicalKeydown` →
  `dispatchTranscludedTaskStartEvent`, gated by `isTranscludedTaskStartKeydown` (`event.key === "@"`). It does a
  **binary** toggle of the source task (` ` ↔ `/`) via `toggleActiveTranscludedTaskStartState` (single) and
  `toggleTranscludedTaskStartStateRange` (counted), using `getNextTranscludedStartToggleSymbol` /
  `isTranscludedStartToggleableStatus`. These helpers are used **only** by the `@` path (verified by usage search);
  `getNextTranscludedStartToggleSymbol` and `isTranscludedStartToggleableStatus` are also re-exported in the
  `module.exports.helpers` block.
- Reusable transcluded plumbing already exists and is well-factored:
  - `getActiveLineTranscludedTaskTarget(editor, sourcePath)` — synchronous; returns the unambiguous embedded block
    candidate on the cursor line (or null).
  - `getTranscludedTaskTargetFromLine(...)` and `collectTranscludedTaskTargetsInLineRange(lines, path, start, end, ch)`.
  - `resolveTranscludedBlockTarget(candidate, context, { taskStatusPredicate })` — resolves the source file/line/status;
    default predicate is `isOpenDoneTaskStatus`.
  - `replaceResolvedTranscludedTaskLine(resolvedTarget, context, forcedNextSymbol)` → editor or vault write, via
    `getNextTranscludedTaskLineText(lineText, line, blockId, forcedNextSymbol)`.
  - **Important gate:** when a `forcedNextSymbol` is given, `getNextTranscludedTaskLineText` calls
    `canForceTranscludedTaskStatus`, which **restricts** transitions (e.g. force `/` only from ` `, force ` ` only from
    `/`, force `x` only from ` `/`/`, force `B`/`-` only from ` `/`x`). This restriction is correct for the old `@`
    toggle and Pomodoro completion, but it is **too narrow for a full forward/backward cycle**, which must be able to
    move a source task from any status to its ring-adjacent status.
  - The underlying rewrite `rewriteTaskLineForTranscludedSource(lineText, nextSymbol, completionDate)` already produces
    the same result as direct-line cycling (it delegates to `rewriteTaskLineForLocalFallback` for Tasks-filter lines,
    which manages the `[completion:: ...]` field on `x`/away-from-`x`). So a cycle write reuses correct metadata
    handling for free.
- Count plumbing already exists and is correct post-fix: `getPendingVimRepeat(cm)` (reads leading digits from
  `inputState.keyBuffer`, falling back to `getRepeat()`), and `resetPendingVimInputState(cm, reason)` (clears the
  pending count so it does not leak into the next Vim command). These are reused unchanged.
- Precedent for capture-phase interception of a key that is **also** a `hotkeys.json` binding: the child-bullet listener
  intercepts `Ctrl+Shift+O` (also bound in `hotkeys.json`) in Vim normal mode and `preventDefault()` +
  `stopImmediatePropagation()` to suppress the command, otherwise falls through. The counted bracket listener follows
  this exact pattern.

## Product Decisions

1. **Remove the `@` keymap completely.** Delete the listener registration, its handlers, and the now-orphaned toggle
   helpers and their exports. There is no vimrc/hotkeys binding for `@`, so nothing outside the plugin changes. Ordinary
   Vim `@<register>` macro playback returns to being fully unshadowed (the plugin no longer listens for `@` at all).

2. **`<option+]>` / `<option+[>` cycle transcluded task block links.** When the active line is a transcluded task block
   link (a list line whose embedded `![[Note#^blockid]]` resolves to a task), the keymap cycles the **source** task's
   status forward (`]`) / backward (`[`) through the same `FIXED_SYMBOLS` ring used for direct tasks — not the old
   binary ` `↔`/` toggle. This means the source task can now be cycled to/from Blocked (`B`), Done (`x`), and Cancelled
   (`-`), exactly like a direct task line, and cycled in both directions.

3. **Per-line precedence: direct checkbox task first, then transcluded link, then existing fallback.** A pure
   transclusion line (`- ![[Note#^id]]`, no checkbox) routes to the transcluded path. A line that is itself a checkbox
   task (`- [ ] ...`, even if it embeds a link) keeps cycling its own local checkbox, unchanged. Non-task, non-link
   lines keep today's single-line plain-bullet formatting fallback. (Minor intentional behavior change: the old `@`
   would toggle the _source_ even from a line that was itself a checkbox-plus-embed; the new keymap treats such a line
   as a direct task. Pure transclusion link lines — the normal case — are unaffected by this nuance.)

4. **Count semantics mirror yesterday's `N@` / `N!`.** An explicit Vim count `N` means "the current line plus `N`
   following physical source lines," clamped at end-of-file. Each line in the range is cycled **independently** by the
   same direction, using per-line precedence (decision 3). Lines in the range with no eligible task/link are skipped. A
   bare (count-less) press keeps single-line behavior. The active line must be eligible (direct cyclable task, or an
   unambiguous transcluded block candidate) before a counted press is consumed; otherwise the press falls through
   (matching the `@`/`!` gating).

5. **Counts apply uniformly to direct and transcluded task lines.** `2<option+]>` on three adjacent plain checkbox tasks
   cycles all three; on three adjacent transcluded task links it cycles all three source tasks; a mixed range cycles
   each line by its own kind. This is the least surprising reading of "the current Obsidian task and the Obsidian tasks
   on the next 2 lines." (If review prefers to scope counts to transcluded links only, the counted range handler can
   drop the direct-line branch with no other changes — see Assumptions.)

6. **Reuse the existing count-read/reset architecture and capture-phase pattern.** Counts cannot come through Obsidian
   commands, so counted bracket handling uses a capture-phase `keydown` listener (like `@`/`!`/child-bullet) that reads
   `getPendingVimRepeat` and calls `resetPendingVimInputState` after consuming. Bare presses are left to the existing
   `hotkeys.json` → command path so single-line behavior and the plain-bullet fallback stay exactly as they are today.

## Behavior Specification

Direction: `<option+]>` = forward (`+1`), `<option+[>` = backward (`-1`).

- **Bare `<option+]>` / `<option+[>`:**
  - Active line is a direct checkbox task → cycle its checkbox (unchanged).
  - Else active line is a transcluded task link resolving to a task → cycle the **source** task's status one step in the
    given direction.
  - Else → existing plain-bullet formatting fallback (unchanged).
- **Counted `N<option+]>` / `N<option+[>` (explicit count `N ≥ 1`):**
  - Only engages when the active line is eligible (direct cyclable task or unambiguous transcluded block candidate).
    When it engages, it consumes the key (so the bare command does not also fire) and clears the pending Vim count.
  - Operates on lines `[cursor.line, cursor.line + N]`, clamped to the document.
  - For each line: direct checkbox task → cycle checkbox; else transcluded task link → cycle source task; else skip.
  - Cursor stays on the original line.
  - When multiple links in the range resolve to the same source `path#^blockid`, that source is cycled once (dedupe),
    matching the old counted `@` behavior.
- **Macro playback:** unaffected — the plugin no longer listens for `@`, and the bracket listener never matches `@`.

## Implementation Approach

All edits are confined to the linked `bob-plugins` source file `plugins/task-status-cycler/main.js`. The implementing
agent must open the linked repo first:

```bash
sase workspace open -p bob-plugins -r "Implement option+[ / option+] transcluded task cycling; remove @ keymap" <workspace_num>
```

(`<workspace_num>` is the number of the primary `bob-cli` workspace the agent is running in.)

### 1. Remove the `@` keymap

- Remove the `this.registerTranscludedTaskStartInputListeners();` call in `onload`.
- Delete `registerTranscludedTaskStartInputListeners`, `handleTranscludedTaskStartPhysicalKeydown`,
  `dispatchTranscludedTaskStartEvent`, `isTranscludedTaskStartKeydown`, and the `handledTranscludedTaskStartEvents`
  WeakSet.
- Delete the now-orphaned `toggleActiveTranscludedTaskStartState` and `toggleTranscludedTaskStartStateRange` methods.
- Delete the now-orphaned module functions `getNextTranscludedStartToggleSymbol` and
  `isTranscludedStartToggleableStatus`, and remove their entries from the `module.exports.helpers` block.
- Keep `getPendingVimRepeat` and `resetPendingVimInputState` (reused by the new counted bracket listener).
- Verify with a usage search that nothing else references any removed symbol before deleting.

### 2. Add a transcluded-cycle write path (relaxed status gate)

The existing `replaceResolvedTranscludedTaskLine` path forbids arbitrary forced transitions via
`canForceTranscludedTaskStatus`. Add a **minimal, additive** way to force any valid ring-adjacent symbol, without
touching the existing `@`/Pomodoro callers:

- Thread an options flag (e.g. `{ allowAnyStatus: true }`) from `replaceResolvedTranscludedTaskLine` through
  `replaceResolvedTranscludedTaskLineInEditor` / `replaceResolvedTranscludedTaskLineInVault` into
  `getNextTranscludedTaskLineText`.
- In `getNextTranscludedTaskLineText`, when `allowAnyStatus` is set, replace the `canForceTranscludedTaskStatus` check
  with a simple guard: the forced symbol must be in `FIXED_SYMBOLS` and differ from the current symbol. Existing callers
  (no flag) keep the current restrictive behavior untouched.
- Add a small predicate `isCyclableTaskStatus(taskStatus)` = task exists and `FIXED_SYMBOLS.includes(symbol)`, used as
  the `taskStatusPredicate` when resolving for a cycle so any real task status resolves (not just open/done).
- Add a method `cycleResolvedTranscludedTaskLink(candidate, context, direction)` that:
  1. `resolveTranscludedBlockTarget(candidate, context, { taskStatusPredicate: isCyclableTaskStatus })`;
  2. computes `nextSymbol = this.getAdjacentSymbol(resolved.taskStatus.symbol, direction)` (null → no-op);
  3. writes via `replaceResolvedTranscludedTaskLine(resolved, context, nextSymbol, { allowAnyStatus: true })`. Build
     `context` as `{ editor, activePath, originPath: activePath }`, exactly like the existing single-line paths.

### 3. Extend the bare cycle command (`handleCycleCommand`) to transcluded links

Insert a transcluded branch between the checkbox branch and the plain-bullet fallback:

- In the `checking` phase, if `getActiveTaskStatus` finds no checkbox, do a **synchronous** eligibility check with
  `getActiveLineTranscludedTaskTarget(editor, activePath)`; if it returns a candidate, return `true` (command enabled).
- In the action phase for that case, fire the async cycle without blocking the command:
  `void this.cycleResolvedTranscludedTaskLink(candidate, context, direction).catch(() => false);` then `return true`.
  This mirrors how the async transcluded open/done path is already bridged elsewhere in the plugin (synchronous
  eligibility check, fire-and-forget async write).
- Leave the checkbox branch and the plain-bullet fallback exactly as they are.

### 4. Add the counted `<option+[>` / `<option+]>` capture listener

Model this directly on the removed `@` listener and the child-bullet listener:

- Register a capture-phase `keydown` listener on `window` and `document` (dedupe with a `WeakSet`), unregistered on
  unload via `this.register(...)`.
- Match direction with a narrow gate: no `ctrl`/`meta`, `alt` true, not `shift`, and physical
  `event.code === "BracketRight"` → forward (`+1`) / `event.code === "BracketLeft"` → backward (`-1`). Use `event.code`
  (layout-independent) because Option+bracket does not yield a plain `[`/`]` `event.key` on macOS. Return null
  otherwise.
- Resolve the focused Markdown view (`getFocusedMarkdownEditorView`) and the normal-mode Vim CM
  (`resolveNormalModeVimCm`); bail (fall through) if not in Vim normal mode, so insert/visual mode and non-editor
  targets keep Obsidian's default handling.
- Read `getPendingVimRepeat(cm)`. If **not** explicit → return without consuming, letting the `hotkeys.json` command run
  the bare single-line behavior from step 3.
- If explicit, compute active-line eligibility synchronously: a direct cyclable task (`getActiveTaskStatus` +
  `isCyclableTaskStatus`) **or** an unambiguous transcluded candidate (`getActiveLineTranscludedTaskTarget`). If neither
  → return without consuming (count falls through, matching `@`/`!`).
- If eligible: mark handled, `preventDefault()` + `stopPropagation()` + `stopImmediatePropagation()`, call
  `resetPendingVimInputState(cm, "counted-cycle-task-status")`, then run the counted range handler (fire-and-forget with
  `.catch`).

Counted range handler (async), mirroring `toggleTranscludedTaskStartStateRange`'s structure:

- Snapshot line texts (`getEditorLineTexts`) and compute `startLine = cursor.line`,
  `endLine = min(startLine + repeat, lastLine)`.
- Iterate `startLine..endLine`; for each line, apply per-line precedence:
  - Direct checkbox task (`getTaskStatusForLine` on the snapshot line, cyclable): compute
    `getAdjacentSymbol(symbol, direction)` and write it. For the **active** line, reuse `setActiveCheckboxStatus`
    (preserves the Tasks-plugin command path for exact single-line parity). For **non-active** lines, use the local
    rewrite path (`setActiveCheckboxStatusLocalWithTaskMetadata` for Tasks-filter lines, else
    `setActiveCheckboxStatusLocal`) because the Tasks-plugin command only acts on the cursor line.
  - Else transcluded task link (`getTranscludedTaskTargetFromLine(line, activePath, lineNo, cursorCh-on-active-only)`):
    dedupe by resolved `path#^blockid`, then `cycleResolvedTranscludedTaskLink(candidate, context, direction)`.
  - Else skip.
- All status writes are single-line, in-place (no line-count change), so snapshot line numbers stay valid across the
  loop. Process sequentially to avoid racing vault writes to the same source file. Keep the cursor on the original line.

## Acceptance Criteria

- The `@` key no longer toggles transcluded source tasks; pressing `@` inserts `@` / drives Vim macro playback normally
  everywhere, including on transcluded task link lines.
- Bare `<option+]>` / `<option+[>`:
  - On a direct checkbox task: cycles its status forward/backward through `[" ", "/", "B", "x", "-"]` (unchanged).
  - On a transcluded task block link: cycles the **source** task's status forward/backward through the same ring,
    including transitions to/from Blocked, Done, and Cancelled, in both directions.
  - On a non-task, non-link list line: still toggles plain-bullet formatting (unchanged).
- Counted `N<option+]>` / `N<option+[>` cycles the current line plus the next `N` lines, each independently, for both
  direct tasks and transcluded task links (mixed ranges included); out-of-range/EOF is clamped; ineligible lines in the
  range are skipped; duplicate source targets are cycled once; the cursor stays put.
- After a handled counted press, the next Vim command (e.g. `j`) is not double-counted.
- Bare presses on non-eligible lines and all existing keymaps (`<Ctrl+Enter>`, `<Enter>`, `<Ctrl+Shift+O>`, open-task
  jumps, `!`, vimrc mappings) are unchanged.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused Node/helper checks (modeling real capture-phase runtime, per the `fix_counted_transclusion_keymaps` lesson —
count lives in `keyBuffer`, `getRepeat()` returns 0 at capture time):

- `getPendingVimRepeat`: `keyBuffer === ["2"]` with `getRepeat() === 0` → `{ repeat: 2, explicit: true }`; empty buffer
  → not explicit (regression guard, unchanged helper).
- Direction gate: `Alt+BracketRight` → forward, `Alt+BracketLeft` → backward; `Ctrl+[`, plain `[`, and `@` → no match.
- `isCyclableTaskStatus`: true for ` `/`/`/`B`/`x`/`-`, false for null/non-task.
- Transcluded cycle write with `{ allowAnyStatus: true }`: `/` → `B` forward, `x` → `B` backward, and same-symbol → no
  write; confirm the restrictive `canForceTranscludedTaskStatus` path is untouched for existing callers.

Manual smoke in Obsidian after `bob plugins sync` + plugin reload (authoritative — this is a runtime event-timing
feature):

1. Bare `<option+]>` / `<option+[>` on a direct checkbox task cycles it both ways through all five statuses.
2. Bare `<option+]>` / `<option+[>` on a transcluded task link cycles the source task both ways, including into Blocked,
   Done, and Cancelled, and back.
3. Three adjacent transcluded task links → `2<option+]>` advances all three source tasks; `2<option+[>` reverses all
   three.
4. Three adjacent direct checkbox tasks → `2<option+]>` advances all three; a mixed direct/link range cycles each by its
   kind.
5. After a handled `2<option+]>`, press `j` → cursor moves exactly once (no count leak).
6. Press `@` on a transcluded task link line → inserts `@` (no status change); `@<register>` and `2@<register>` macro
   playback still work.

Deploy from the `bob-plugins` source repo (per the workspace deploy convention, pass `-r "$PWD"`):

```bash
bob plugins sync --plugin task-status-cycler --repo "$PWD"
```

Do not edit the deployed copy under the vault directly.

## Risks and Mitigations

- **Risk:** relaxing the transcluded status gate could let unintended forced transitions through elsewhere.
  - _Mitigation:_ gate the relaxation behind an explicit `{ allowAnyStatus: true }` flag used only by the new cycle
    path; existing `@`-legacy/Pomodoro callers keep `canForceTranscludedTaskStatus` untouched.
- **Risk:** counted bracket handling depends on Vim internals (`keyBuffer`) and capture-phase ordering.
  - _Mitigation:_ reuse the already-fixed `getPendingVimRepeat` / `resetPendingVimInputState` and the proven
    window+document capture pattern from `@`/child-bullet; require the in-Obsidian manual smoke as the final gate.
- **Risk:** the Tasks-plugin command only acts on the cursor line, so counted direct-line cycling could touch the wrong
  line.
  - _Mitigation:_ use the Tasks-command path only for the active line; use the line-addressed local rewrite for
    non-active lines in the range.
- **Risk:** consuming a counted press without clearing the count leaks it into the next command.
  - _Mitigation:_ call `resetPendingVimInputState` immediately after consuming; cover with manual step 5.
- **Risk:** dead code / stale exports after removing `@`.
  - _Mitigation:_ usage-search every removed symbol and drop its `module.exports.helpers` entry; `node --check` and
    `npm run validate` after.
- **Risk:** source/deployed plugin drift.
  - _Mitigation:_ edit linked source only, deploy with `bob plugins sync`, verify the targeted diff after deployment.

## Assumptions / Scope Decisions to Confirm

- **Full cycle vs. old binary toggle (Decision 2):** assumed `<option+[>`/`<option+]>` should run the _full_
  `FIXED_SYMBOLS` cycle on transcluded source tasks (consistent with their behavior on direct tasks), replacing the old
  ` `↔`/` toggle. If instead only an open↔in-progress toggle is wanted on transcluded links, the write step becomes the
  old toggle symbol rather than `getAdjacentSymbol` — a small localized change.
- **Counts on direct task lines (Decision 5):** assumed counts apply to both direct and transcluded task lines. If
  counts should apply to transcluded links only (strict `N@` parity), drop the direct-line branch from the counted range
  handler; nothing else changes.
