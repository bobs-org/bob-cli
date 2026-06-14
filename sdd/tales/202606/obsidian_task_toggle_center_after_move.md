---
create_time: 2026-06-14 11:11:47
status: wip
prompt: sdd/prompts/202606/obsidian_task_toggle_center_after_move.md
---

# Plan: Center Moved Obsidian Task Toggle Line After Section Routing

## Context

The current `<Ctrl+Shift+]>` behavior lives in the Bob vault plugin, not in the Rust CLI:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`

The command is already registered as `task-status-cycler:toggle-obsidian-task` and the Vim normal-mode action routes to
the same plugin method through `taskStatusCyclerToggleObsidianTask`. The previous approved change added same-file
routing:

- A Future Work bullet promoted into a proper Obsidian task is moved to the bottom of `Tasks`.
- A proper Obsidian task demoted into a normal bullet is moved to the bottom of `Future Work`.
- If either section is missing, the plugin keeps the old in-place line rewrite behavior.

The new move path currently sets the cursor on the moved converted line and then calls
`editor.scrollIntoView({ from, to })`. That only reveals the line if needed; it does not guarantee a Vim `zz` style
redraw with the cursor line centered in the viewport.

Useful local precedent exists in other vault plugins:

- `bob-ledger-tools` resolves the underlying CodeMirror 6 `EditorView` from Obsidian editor shapes with
  `cm.cm6 || cm.cm || cm`, converts line/ch coordinates to a document offset, and dispatches
  `EditorView.scrollIntoView(offset, { y: "center", x: "nearest" })`.
- `bob-navigation-hotkeys` uses `EditorView.scrollIntoView(offset, { y: "start" })` for Vim `zt` behavior.
- The task-status-cycler plugin currently does not import `@codemirror/view`, but sibling vault plugins already do.

This follow-up should be a narrow redraw fix, not a change to task routing, task-line conversion, command registration,
Vim mappings, `hotkeys.json`, or the plugin manifest.

## Goal

After `<Ctrl+Shift+]>` moves a task or bullet to another section, redraw the editor with the moved converted line
centered vertically in the visible editor pane, matching Vim `zz` behavior. The cursor should remain on the moved line
at the same column already computed by `getObsidianTaskToggleCursorCh()`.

## Behavior Specification

1. Center only after a routing move plan succeeds (`plan.mode === "move"`).
2. Do not change in-place toggle behavior when routing is inactive, when one of the sections is missing, or when the
   active line is not eligible for routing.
3. Preserve the current cursor placement:
   - target line is `plan.cursorLine`;
   - target column is `plan.cursorCh`, clamped to the final line length.
4. After cursor placement, use the underlying CodeMirror 6 view when available:
   - convert `{ line, ch }` to a CM6 document offset;
   - dispatch `EditorView.scrollIntoView(offset, { y: "center", x: "nearest" })`.
5. If CM6 is unavailable or the helper fails, fall back to Obsidian's editor-level reveal/center behavior:
   - try `editor.scrollIntoView({ from: position, to: position }, true)`;
   - then try the current one-argument `editor.scrollIntoView({ from, to })` shape. Cursor placement should still count
     as success even if scrolling cannot be centered.
6. Both invocation paths should behave the same:
   - the regular hotkey path already receives `(editor, view)` in `handleToggleObsidianTaskCommand`;
   - the Vim action re-fetches the active `MarkdownView`, so it can pass the same view context into the toggle method.

## Implementation

Edit only `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.

1. Add `const { EditorView } = require("@codemirror/view");` at the top, matching sibling vault plugins.

2. Add small defensive editor-view helpers near the other editor helpers and export the pure/testable ones through
   `module.exports.helpers`:
   - `getEditorViewFromEditor(editorOrCm)`:
     - resolve `editorOrCm.cm6 || editorOrCm.cm || editorOrCm`;
     - require `state.doc` and `dispatch`;
     - return `null` if the shape is missing.
   - `editorViewPositionFromLineCh(editorView, line, ch)`:
     - clamp zero-based line/ch against `editorView.state.doc`;
     - use `doc.line(safeLine + 1)` because CM6 line numbers are one-based;
     - return the absolute document offset, or `null` on failure.
   - `centerEditorViewOnPosition(editorView, line, ch)`:
     - call `editorViewPositionFromLineCh`;
     - dispatch `EditorView.scrollIntoView(position, { y: "center", x: "nearest" })`;
     - return `true`/`false`, never throw.
   - `centerEditorLineInView(editor, line, ch, markdownView)`:
     - resolve the CM6 view from `editor`, then from `markdownView.editor` as a fallback;
     - call `centerEditorViewOnPosition`;
     - if that fails, fall back to the existing editor-level `scrollIntoView` shapes, preferring the two-argument
       centered form.

3. Thread the active view through the Obsidian task toggle command path:
   - `handleToggleObsidianTaskCommand(checking, editor, view)` should call
     `this.toggleActiveObsidianTask(editor, obsidianTaskToggle, view)`.
   - `handleVimToggleObsidianTask()` should call `this.toggleActiveObsidianTask(view.editor, undefined, view)`.
   - Keep `toggleActiveObsidianTask(editor, plan = ..., view = null)` backward-compatible for tests and direct callers.
   - Pass `view` into `applyObsidianTaskMovePlan(editor, plan, view)`.

4. Replace the move path's current generic reveal call with the centering helper:
   - after applying the document line replacement;
   - after `editor.setCursor({ line: targetLine, ch: targetCh })`;
   - call `this.centerEditorLineInView(editor, targetLine, targetCh, view)`.

5. Leave these paths unchanged:
   - `applyObsidianTaskReplacePlan`;
   - section detection and insertion planning helpers;
   - Vim mapping registration;
   - `manifest.json`;
   - `.obsidian/hotkeys.json`.

No bounded scroll reassertion should be added initially. The dash/`zt` helper needed a retry because it races file-open
scroll restoration. This task operates in the already-active editor after a same-document edit, so a single CM6
`scrollIntoView(..., { y: "center" })` after cursor placement should be the final scroll instruction. If manual testing
shows Obsidian still clobbers the center after the synchronous turn, the follow-up escalation is a one-frame
`requestAnimationFrame` re-center, not an eight-frame file-open style loop.

## Validation

1. Syntax and diff checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob status --short` before and after, confirming unrelated dirty files remain untouched.

2. Focused Node checks with mocked `obsidian` and `@codemirror/view` modules:
   - `getEditorViewFromEditor` resolves an Obsidian editor (`editor.cm`), a Vim adapter shape (`cm.cm6`), and a raw
     EditorView shape.
   - `editorViewPositionFromLineCh` clamps line/ch and returns the correct document offset for a mock CM6 doc.
   - `centerEditorViewOnPosition` dispatches one effect built by mocked `EditorView.scrollIntoView` with
     `{ y: "center", x: "nearest" }`.
   - `centerEditorViewOnPosition` returns `false` without throwing when the view shape is incomplete or `doc.line`
     throws.
   - `applyObsidianTaskMovePlan` sets the cursor, then attempts centering on the moved target line.
   - `applyObsidianTaskReplacePlan` does not call the centering helper.
   - Existing task-routing helper checks from the prior change still pass at least for sibling sections, nested
     `### Future Work`, and missing-section in-place fallback.

3. Manual smoke test after reloading Obsidian or toggling `task-status-cycler`:
   - In a scratch note with sibling `## Tasks` and `## Future Work`, put the cursor on a Future Work bullet near the
     bottom of a long note and press `<Ctrl+Shift+]>`; the converted task lands in `Tasks`, the cursor follows it, and
     the moved line is centered like Vim `zz`.
   - Convert that task back; it lands in `Future Work` and is centered.
   - Repeat in a note whose `Future Work` heading is nested under `Tasks`.
   - Repeat from Vim normal mode to confirm the Vim mapping and hotkey path both pass the view context correctly.
   - Verify a note missing either section still rewrites in place and does not unexpectedly recenter the viewport.

## Risks

- `@codemirror/view` is a new import for this plugin, but it is already used by other local Obsidian plugins and is the
  right API for exact vertical alignment.
- Obsidian's editor abstraction shapes can drift across versions. The helper should feature-detect and fall back instead
  of assuming `editor.cm` always exists.
- A centered redraw at the very start or end of a file can only center as much as the scroll range allows; this matches
  normal editor behavior.
- Overusing centering would be disruptive, so the implementation should only call it for the routed move path and leave
  in-place toggles alone.
