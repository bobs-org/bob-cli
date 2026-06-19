---
create_time: 2026-06-19 08:25:14
status: wip
prompt: sdd/prompts/202606/obsidian_open_task_zz_center.md
---
# Plan: Center Ctrl+Shift+J/K Open-Task Jumps Like Vim zz

## Goal

Make the existing `<Ctrl+Shift+J>` and `<Ctrl+Shift+K>` open-task navigation redraw the active editor with the jumped-to
task line vertically centered, matching Vim `zz`.

The intended behavior:

- `<Ctrl+Shift+J>` still jumps to the next open Obsidian `#task`, wrapping to the first matching task when needed.
- `<Ctrl+Shift+K>` still jumps to the previous open Obsidian `#task`, wrapping to the last matching task when needed.
- After a successful jump, the cursor lands on the target task line at column 0 as it does today.
- After that cursor move, the visible editor pane centers the current line vertically, subject to normal scroll limits
  near the start or end of the file.
- Failed jumps remain unchanged: no cursor move, no scroll scheduling, and the existing direction-specific notice is
  shown.

## Context Reviewed

- Required Obsidian long-term memory was read through:
  `sase memory read obsidian.md --reason "Need Obsidian vault and plugin workflow context before planning navigation hotkey redraw behavior"`.
- This is live Obsidian vault work under `/home/bryan/bob`, not Rust `bob-cli` work. No CLI subcommands or options are
  being added, so the CLI rules memory does not apply.
- `/home/bryan/bob/AGENTS.md` requires checking vault status before edits, preserving unrelated dirty files, and
  committing task-related vault edits with `/sase_git_commit` before terminating after implementation.
- Current vault status includes unrelated dirty notes and an uncommitted change in
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`. That plugin diff appears to be the open-task
  same-dispatch deduplication guard from the recent notice-dedup plan. Treat it as user-owned/current work: do not
  revert or overwrite it, and re-read the diff before implementation.
- The relevant open-task code is in `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- `jumpToOpenObsidianTask(editor, direction)` currently:
  - gets the current cursor;
  - uses `getOpenObsidianTaskJumpLine(...)` for circular task selection;
  - sets the cursor with `setEditorCursor(editor, { line: targetLine, ch: 0 })`;
  - then calls `scrollEditorLineToTop(editor, targetLine)`.
- `setEditorCursor(...)` already asks Obsidian to center the cursor line with `editor.scrollIntoView(..., true)`, but
  the later `scrollEditorLineToTop(...)` explicitly changes the final scroll result to Vim `zt`-style top alignment.
- `scrollEditorLineToTop(...)` is still needed for section-header and dash-task behavior. The open-task change should
  stop using it for successful open-task jumps, not change its helper semantics globally.
- Existing vault precedent for reliable `zz` behavior is to dispatch CodeMirror 6
  `EditorView.scrollIntoView(position, { y: "center", x: "nearest" })`, and to defer that centered scroll by one frame
  when a Vim-mode command path could otherwise be followed by a trailing cursor-visibility scroll.

## Product Decisions

1. The task selection semantics do not change. Open-task parsing, frontmatter/fence skipping, circular wrap behavior,
   and no-target notice conditions remain as they are today.

2. Centering happens only after a successful open-task jump. The no-target cases should not redraw the editor.

3. Section-header navigation remains Vim `zt`-style top aligned. This request is specifically about the open-task
   `<Ctrl+Shift+J/K>` keymaps.

4. Keybinding surfaces stay stable:
   - no `.obsidian/hotkeys.json` changes;
   - no `obsidian_vimrc.md` changes;
   - no command id changes;
   - keep the existing capture-phase Vim-normal fallback.

5. Use a small, scoped helper for center alignment rather than changing `setEditorCursor(...)`, because that helper is
   shared by other plugin features that expect the current behavior.

6. Defer the final center operation one animation frame for all successful open-task jumps. This keeps insert-mode and
   non-Vim behavior correct and makes Vim-normal behavior robust if any later editor visibility scroll occurs in the
   same keydown turn.

## Implementation Approach

Edit only:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`

### 1. Add a CM6 center helper beside the top-scroll helper

Add a defensive helper near `scrollEditorLineToTop(editor, line)`, for example
`scrollEditorLineToCenter(editor, line, ch = 0)`.

The helper should:

- resolve the existing CodeMirror view through `editor.cm`, mirroring the local `scrollEditorLineToTop` pattern;
- require `cm.dispatch`, `cm.state.doc.line`, `EditorView`, and `EditorView.scrollIntoView`;
- normalize zero-based `line` and `ch`;
- convert the target to a CM6 document offset using `cm.state.doc.line(line + 1)`;
- clamp `ch` within the line bounds when possible;
- dispatch `EditorView.scrollIntoView(offset, { y: "center", x: "nearest" })`;
- return `true` on success and `false` on unsupported editor shapes or exceptions;
- never throw.

Keep `scrollEditorLineToTop` unchanged for existing callers.

### 2. Add a deferred open-task center scheduler

Use the existing `deferToNextFrame(callback)` and `cancelDeferred(deferred)` helpers already present in
`bob-navigation-hotkeys/main.js`.

Add a narrow scheduler such as `scheduleOpenTaskJumpCenter(plugin, editor, line, ch = 0)`:

- cancel any existing `plugin.pendingOpenTaskJumpCenterDeferred`;
- schedule one next-frame callback;
- clear the pending handle before running;
- first attempt `scrollEditorLineToCenter(editor, line, ch)`;
- if CM6 centering is unavailable, fall back to `editor.scrollIntoView({ from: position, to: position }, true)` and then
  the one-argument shape, matching the existing defensive style;
- treat scrolling as best-effort: a failed center must not turn a successful jump into a command failure.

Initialize `this.pendingOpenTaskJumpCenterDeferred = null` in `onload()`, and cancel it in the existing registered
cleanup callback along with `cancelPendingRestore()` and `cancelPendingDashTasksJump()`.

One deferred frame is enough for this command because the editor is already open and active. Do not add a multi-frame
assertion loop unless manual testing later proves Obsidian still overrides the center.

### 3. Replace only the open-task final scroll

In `jumpToOpenObsidianTask(editor, direction)`, after successful `setEditorCursor(editor, { line: targetLine, ch: 0 })`:

- replace `scrollEditorLineToTop(editor, targetLine)` with the new deferred center scheduler;
- leave the method's boolean result and notice behavior unchanged;
- leave the same-dispatch open-task deduplication guard intact if it is still present in the working tree.

Do not change `jumpToSectionHeader(...)`, `jumpToActiveDashTasks(...)`, or any other existing `scrollEditorLineToTop`
caller.

### 4. Preserve and account for current dirty state

Before implementation, re-run:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

If the current uncommitted deduplication guard is still present, build on top of it. If the file changed again, re-read
the affected region before editing.

## Validation Plan

Run static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Run focused throwaway Node tests with stubbed `obsidian` and `@codemirror/view`, then remove the temporary files.

Cover helper behavior:

- `scrollEditorLineToCenter` dispatches one `EditorView.scrollIntoView` effect with `{ y: "center", x: "nearest" }`.
- The helper computes the expected offset for zero-based line/ch input.
- The helper clamps or rejects invalid input without throwing.
- The helper returns `false` when `editor.cm`, `dispatch`, `state.doc.line`, or `EditorView.scrollIntoView` is missing.
- The deferred scheduler cancels a stale pending center before scheduling a new one.
- The deferred scheduler uses the editor-level centered `scrollIntoView` fallback when CM6 centering is unavailable.

Cover open-task command behavior:

- Successful next jump from a lower task to a wrapped first task sets the cursor to `{ line: firstTaskLine, ch: 0 }` and
  schedules a deferred center.
- Successful previous jump from an upper task to a wrapped last task sets the cursor to `{ line: lastTaskLine, ch: 0 }`
  and schedules a deferred center.
- After draining a fake requestAnimationFrame queue, the successful jump dispatches a center effect, not a
  `{ y: "start" }` top-scroll effect.
- Zero-task and one-task/current-line cases emit the existing notice, leave the cursor untouched, and schedule no
  center.
- Existing circular `getOpenObsidianTaskJumpLine(...)` parser and wrap cases still pass.
- If the same-dispatch deduplication guard is present, keep or extend its regression checks so a duplicate physical
  keydown does not schedule two centers or move twice.

Manual smoke test after reloading Obsidian or toggling the plugin:

1. In a long note with multiple open `#task` lines, put the cursor on the last matching task and press `<Ctrl+Shift+J>`
   in Vim normal mode. The cursor should wrap to the first task and the line should settle vertically centered.
2. Put the cursor on the first matching task and press `<Ctrl+Shift+K>`. The cursor should wrap to the last task and
   center.
3. Repeat the same two checks in insert mode or with Vim mode inactive, so the Obsidian hotkey path is covered.
4. In a one-task note, pressing either chord from another line should jump to and center that task.
5. In zero-task and one-task/current-line cases, the existing notice should appear and the viewport should not jump.
6. Confirm `<Ctrl+J>` and `<Ctrl+K>` section-header jumps still use top alignment.

Review final diff:

```bash
git -C /home/bryan/bob diff -- .obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob status --short
```

## Commit And Hygiene

If implementation is approved later:

- stage only `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`;
- leave unrelated dirty notes, `task-status-cycler/main.js`, `obsidian_vimrc.md`, memory files, and untracked daily
  notes untouched unless the user explicitly redirects;
- commit the task-related vault change with `/sase_git_commit`, per `/home/bryan/bob/AGENTS.md`.

## Risks And Mitigations

- **Removing needed top alignment elsewhere:** only replace the open-task call site; keep `scrollEditorLineToTop`
  unchanged for section headers and dash tasks.
- **Vim or Obsidian overriding a synchronous center:** schedule the center for the next frame so it runs after the
  current keydown/editor command turn.
- **Stale deferred center after rapid repeated key presses:** track one pending open-task center and cancel it before
  scheduling another.
- **Unsupported editor shape in a future Obsidian version:** feature-detect CM6 and fall back to Obsidian's editor-level
  centered scroll.
- **Current dirty plugin diff:** re-read and preserve it before implementing; do not revert user-owned changes.
- **Near file boundaries:** true visual centering is constrained by normal editor scroll limits, matching Vim behavior.
