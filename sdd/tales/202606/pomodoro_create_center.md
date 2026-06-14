---
create_time: 2026-06-14 13:15:30
status: done
prompt: sdd/prompts/202606/pomodoro_create_center.md
---

# Plan: Center the Newly Created Pomodoro After the Completion Keymap

## Goal

After the Pomodoro completion keymap creates a fresh `- [ ] ()` Pomodoro task and moves the cursor to that new
placeholder, redraw the editor so the current line is vertically centered in the pane, matching Vim `zz`.

The visible result should be:

- complete the current open Pomodoro task;
- insert the new placeholder Pomodoro below the completed Pomodoro's sub-bullet block;
- carry forward the existing eligible sub-bullets exactly as today;
- place the cursor on the new placeholder line between the parens, exactly as today;
- then center that cursor line in the editor viewport.

## Context Reviewed

- Read `memory/short/sase.md`: this work runs from an ephemeral `bob-cli_<N>` workspace; do not run project-sensitive
  commands outside the intended repo without reason.
- Read Obsidian long-term memory through the required audited command:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and vim-mode keymap context before planning Shift+Enter Pomodoro task centering"`.
- Read `/home/bryan/bob/AGENTS.md`: the live vault may already have legitimate dirty files; inspect status before
  editing; do not stage/revert/overwrite unrelated changes; commit only task-related vault changes if implementation is
  later approved.
- Checked `/home/bryan/bob` status. Current unrelated dirty files are notes only: `2026/20260614.md`, `bob.md`,
  `dev.md`, and `sase.md`. The target plugin file is clean at inspection time.
- Inspected the live vault implementation: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- Inspected the existing centered Pomodoro jump implementation:
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- Reviewed related SDD tales:
  - `sdd/tales/202606/pomodoro_ctrl_enter_close_and_create.md`
  - `sdd/tales/202606/pomodoro_next_cursor_jump.md`
  - `sdd/tales/202606/always_create_pomodoro_task_1.md`
  - `sdd/tales/202606/pomodoro_jump_center_fix.md`
  - `sdd/tales/202606/pomodoro_jump_center_regression.md`
  - `sdd/tales/202606/obsidian_task_toggle_center_after_move.md`
  - `sdd/tales/202606/dash_tasks_zt_redraw_fix.md`

## Current State

The Pomodoro completion flow lives in `task-status-cycler/main.js`.

- Vim normal-mode `<C-CR>` / `<C-Enter>` are mapped to `taskStatusCyclerToggleTaskOpenDone`.
- `handleVimTaskToggleOpenDone()` detects an open top-level Pomodoro task inside the `## Pomodoros` section and calls
  `completeActivePomodoroTask(view.editor, activeFile, pomodoroContext)`.
- `completeActivePomodoroTask()` completes transcluded sub-bullet source tasks, recomputes editor lines, builds a local
  plan with `buildPomodoroCompletionPlan()`, and applies it with `applyPomodoroCompletionPlan()`.
- The current `buildPomodoroCompletionPlan()` already always inserts a fresh placeholder Pomodoro at
  `sourceRange.endLine`, carries forward non-transcluded task-link bullets, and returns `cursorTargetLine` pointing at
  that new placeholder.
- `applyPomodoroCompletionPlan()` applies edits, sets the cursor to `cursorTargetLine`, computes the placeholder-aware
  column with `getPomodoroCursorTargetCh()`, then calls `editor.scrollIntoView({ from, to })`.

That last scroll only reveals the line. It does not ask CodeMirror to center the line, and it runs synchronously inside
the Vim command path. Prior Pomodoro jump work showed that synchronous scrolls can be clobbered by codemirror-vim or
Obsidian's final cursor-visibility scroll. The proven pattern is to place the cursor first, then defer the centered
scroll so it is the final scroll instruction.

## Keymap Naming Note

The prompt names the user-facing keymap as `<shift+enter>`. In the inspected live code, the Pomodoro completion behavior
is owned by the Vim normal-mode `<C-CR>` / `<C-Enter>` mapping. I did not find a separate Shift+Enter mapping in
`task-status-cycler/main.js` or `.obsidian/hotkeys.json`.

This plan targets the shared Pomodoro completion behavior rather than changing hotkey registration. If implementation
inspection later reveals an additional Shift+Enter path outside this plugin, it should route to the same centering code
instead of duplicating logic.

## Design

Use the same CodeMirror 6 centered-scroll approach already used elsewhere in the vault, but apply it to the Pomodoro
completion target after the new placeholder is inserted.

1. Keep the existing Pomodoro completion text edits unchanged.
   - Do not alter transcluded task completion.
   - Do not alter carry-forward bullet classification.
   - Do not alter the "always create a fresh placeholder" behavior.
   - Do not alter cursor placement between the placeholder parens.

2. Replace the immediate plain reveal in `applyPomodoroCompletionPlan()` with a deferred `zz`-style center.
   - After edits are applied and `editor.setCursor({ line: targetLine, ch: clampedTargetCh })` succeeds, schedule a
     centered redraw for that same target line/ch.
   - Prefer the CM6 `EditorView.scrollIntoView(position, { y: "center", x: "nearest" })` path through the plugin's
     existing `centerEditorViewOnPosition()` helper.
   - Fall back to the existing editor-level `scrollIntoView` shapes only when CM6 centering cannot run.

3. Defer the centered scroll past the Vim command turn.
   - Add small local defer helpers matching the proven `bob-ledger-tools` pattern: `deferToNextFrame(callback)` using
     `window.requestAnimationFrame` with `setTimeout(..., 0)` fallback, and `cancelDeferred(deferred)`.
   - Track one pending center handle on the plugin instance, for example `this.pendingPomodoroCenterDeferred`.
   - Cancel any previous pending center before scheduling a new one, so repeated key presses do not stack stale scrolls.
   - Cancel the pending handle in `onunload()`.

4. Use a bounded retry only for editor-view readiness, not for fighting the user.
   - `task-status-cycler` already operates on the active editor, so one deferred frame is likely enough.
   - Still follow the safer ledger precedent with a small attempt budget, e.g. `CENTER_ON_LINE_ATTEMPTS = 5`, so the
     code can wait briefly if the editor view is temporarily unavailable after the edit.
   - Each attempt should re-resolve the target editor view from the editor or passed `MarkdownView`.
   - If no CM6 view is available by the final attempt, perform the editor-level reveal fallback once.

5. Pass the active view through the Pomodoro path.
   - Change `handleVimTaskToggleOpenDone()` to call
     `completeActivePomodoroTask(view.editor, activeFile, pomodoroContext, view)`.
   - Add an optional `markdownView = null` parameter to `completeActivePomodoroTask()`.
   - Pass that optional view to `applyPomodoroCompletionPlan(editor, plan, cursor, markdownView)`.
   - Keep defaults backward-compatible for existing tests or direct calls.

6. Center only the Pomodoro-completion target.
   - The center should run when `applyPomodoroCompletionPlan()` has a valid `plan.cursorTargetLine`.
   - This effectively means the newly created Pomodoro placeholder in current behavior.
   - Done-Pomodoro reopen, non-Pomodoro task toggles, transcluded sub-bullet toggles, plain task-status cycling, and
     other keymaps should keep their current scrolling behavior.

## Implementation Scope

Expected vault file to edit:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No expected edits:

- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/obsidian_vimrc.md`
- vault notes, templates, memory files, or `bob-cli` Rust/script files

## Implementation Steps

1. Add a local center-defer constant and helpers near the existing editor-view helpers:
   - `CENTER_ON_LINE_ATTEMPTS = 5`
   - `deferToNextFrame(callback)`
   - `cancelDeferred(deferred)`

2. Initialize and clean up pending center state:
   - in `onload()`, set `this.pendingPomodoroCenterDeferred = null`;
   - in `onunload()`, cancel it if present and clear the field.

3. Add a method such as `scheduleCenterEditorLineInView(editor, line, ch, markdownView = null, options = {})`.
   - Cancel any existing pending center.
   - Defer the first attempt to the next frame.
   - On each attempt, try `centerEditorViewOnPosition(getEditorViewFromEditor(editor), line, ch)`.
   - If that fails because the target view is unavailable and attempts remain, schedule the next frame.
   - Then try `centerEditorViewOnPosition(getEditorViewFromEditor(markdownView.editor), line, ch)` if a view was passed.
   - Finally call the existing `centerEditorLineInView(editor, line, ch, markdownView)` as the fallback.
   - Ensure a successful CM6 center returns without running a later nearest-scroll fallback.

4. Thread `markdownView` through the Pomodoro completion path:
   - `handleVimTaskToggleOpenDone()` passes the active `view`.
   - `completeActivePomodoroTask()` accepts the optional view and forwards it.
   - `applyPomodoroCompletionPlan()` accepts the optional view.

5. In `applyPomodoroCompletionPlan()`, after setting the cursor to `targetLine`/`clampedTargetCh`:
   - remove the current immediate one-argument `editor.scrollIntoView({ from, to })` block;
   - call `this.scheduleCenterEditorLineInView(editor, targetLine, clampedTargetCh, markdownView)`.

6. Export only genuinely testable helpers if needed.
   - Existing exports already include `getEditorViewFromEditor`, `editorViewPositionFromLineCh`, and
     `centerEditorViewOnPosition`.
   - Export `deferToNextFrame` / `cancelDeferred` only if the focused Node checks need direct access; otherwise test via
     `applyPomodoroCompletionPlan()` on a plugin instance with a fake rAF queue.

## Acceptance Criteria

Given a daily-style Pomodoros section where the cursor is on an open Pomodoro:

```md
- [ ] (**1205-1230** [t:: 25m])
  - [[bob#^close-and-create-pom-task]]
```

pressing the Pomodoro completion keymap produces the same text as today:

```md
- [x] (**1205-1230** [t:: 25m])
  - [[bob#^close-and-create-pom-task]]
- [ ] ()
  - [[bob#^close-and-create-pom-task]]
```

and additionally:

- the cursor is on the new `- [ ] ()` line, between the parens;
- the editor viewport redraws with that current line vertically centered, as with Vim `zz`;
- the behavior works when the new placeholder would otherwise be near the top or bottom edge of the viewport;
- transcluded sub-bullets still complete in their source files before the placeholder is inserted;
- copyable non-transcluded task-link bullets are still copied under the new placeholder;
- note-only and transcluded bullets are still not copied;
- reopening a done Pomodoro does not create or center a new placeholder;
- non-Pomodoro task toggles, transcluded sub-bullet toggles, plain checkbox toggles, and other Vim mappings keep current
  behavior.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob status --short
```

Focused Node checks with stubbed `obsidian` and `@codemirror/view` modules:

- Existing `buildPomodoroCompletionPlan()` behavior remains unchanged for fresh placeholder insertion and
  `cursorTargetLine`.
- `applyPomodoroCompletionPlan()` still applies the expected text edits and sets the cursor to the placeholder line
  between the parens.
- After cursor placement, the Pomodoro apply path schedules a deferred center instead of immediately calling the plain
  one-argument reveal.
- Draining a fake `requestAnimationFrame` queue dispatches
  `EditorView.scrollIntoView(position, { y: "center", x: "nearest" })` for the placeholder line.
- If the target editor view is missing for the first frame and appears on a later frame, centering retries and then
  succeeds.
- If no CM6 editor view is available, the fallback editor-level scroll path runs once and does not throw.
- Non-Pomodoro toggle paths do not schedule the Pomodoro center.

Manual smoke test after reloading the plugin in Obsidian:

- In a long scratch daily-style note, complete an open Pomodoro whose new placeholder would appear near the bottom of
  the pane; confirm the cursor lands between the parens and the line is centered.
- Repeat with the Pomodoro above the current viewport or near the top edge.
- Complete a Pomodoro with transcluded and non-transcluded sub-bullets; confirm source-task completion and copied-bullet
  behavior did not regress.
- Reopen a done Pomodoro and confirm no placeholder is created and no unexpected centering occurs.

## Risks

- A synchronous center would likely be clobbered by the same Vim/Obsidian trailing scroll behavior already diagnosed in
  `bob-ledger-tools`; deferring the center is the main mitigation.
- Repeated frame assertions could fight immediate user input. Keep the retry budget small, cancel stale pending centers,
  and only retry while the editor view is not ready.
- At the very beginning or end of a document, true visual centering is limited by normal scroll bounds; this matches Vim
  and CodeMirror behavior.
- The prompt's key name differs from the inspected mapping. The implementation should target the Pomodoro completion
  behavior and avoid hotkey churn unless a separate Shift+Enter binding is found during implementation.
