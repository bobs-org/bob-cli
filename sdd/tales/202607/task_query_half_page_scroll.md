---
create_time: 2026-07-07 19:24:59
status: wip
prompt: sdd/prompts/202607/task_query_half_page_scroll.md
---
# Plan: Natural Ctrl-d/u Scrolling Through Rendered Tasks Queries

## Context

The failing behavior is owned by the `task-status-cycler` Obsidian plugin in the `bob-plugins` linked repo, not by
`bob-cli` Rust code.

Current state:

- `plugins/task-status-cycler/main.js` registers Vim normal-mode `<C-d>` and `<C-u>`.
- The handler computes a half-page count in source lines, jumps the cursor by that many lines, then skips source query
  fences with `findNearestNonQueryLine()`.
- That works for avoiding source `tasks`/`dataview`/`dataviewjs` query lines, but it does not account for Live Preview
  rendered height. A small source fence can render into a long Tasks result list, so the cursor jump can leap from
  before the fence to after it and the viewport appears to skip directly to the bottom of the rendered todos.
- Native scroll-only mappings such as `<C-e>` and `<C-y>` are also not enough because the cursor eventually reaches the
  query source line, causing the rendered Tasks block to unrender.

Relevant Tasks plugin DOM facts:

- The Tasks code block processor uses a `block-language-tasks` render container.
- Rendered query result lists use `ul.plugin-tasks-query-result`.
- Rendered task rows use `li.task-list-item.plugin-tasks-list-item`.
- The row `data-line` is a result index, not a reliable source line number, so the fix should not rely on it for cursor
  placement.

## Goal

Make Vim normal-mode `<C-d>` and `<C-u>` feel natural when browsing a rendered Tasks query:

- Repeated presses should scroll through visible task results by approximately a half viewport, not jump past the whole
  rendered list.
- The cursor must not land on any source line inside a `tasks`, `dataview`, or `dataviewjs` query fence.
- The Tasks block must stay rendered while browsing it.
- Existing non-query behavior should remain as close as possible to the current source-line half-page movement.

## Scope

Primary file:

- `bob-plugins/plugins/task-status-cycler/main.js`

Deployment after implementation:

- Run `bob plugins sync -p task-status-cycler` so the source-of-truth plugin change is deployed to `~/bob`.

Out of scope unless implementation proves otherwise:

- No `bob-cli` Rust changes.
- No changes to `obsidian_vimrc.md`; the plugin already owns `<C-d>`/`<C-u>`.
- No changes to the upstream Tasks plugin.
- No memory-file edits.

## Design

### 1. Keep the existing Vim mapping ownership

Keep `<C-d>` and `<C-u>` registered in `task-status-cycler`. This is already the right place because it owns the current
half-page skip-query behavior and other Vim normal-mode editing customizations.

The action should continue to receive the CodeMirror/Vim adapter object and choose between two paths:

1. Rendered Tasks-query scroll path.
2. Existing source-line cursor movement path.

### 2. Add rendered Tasks query detection

Resolve the underlying CM6 `EditorView` with the existing `getEditorViewFromEditor(cm)` helper. From that view:

- Use `editorView.scrollDOM` as the scroll container.
- Search inside the editor DOM for rendered Tasks query elements:
  - Prefer containers around `ul.plugin-tasks-query-result`.
  - Walk to the closest `.block-language-tasks` container when present.
  - Treat a query container as active when it intersects the editor viewport or is the nearest rendered Tasks block in
    the scroll direction.

This makes the behavior depend on rendered DOM height rather than source fence height.

### 3. Scroll rendered query results by pixels, not source lines

When a rendered Tasks query context is active:

- Do not move the cursor as the primary operation.
- Scroll `editorView.scrollDOM` by about half of its visible height in the requested direction.
- Clamp the scroll target so a single press cannot skip from before a rendered query to after it without showing the
  query body.
- At the top or bottom edge of the active rendered query, allow normal movement to resume only after the rendered query
  has been visibly exhausted.

This matches the user's intended browsing model: the cursor remains parked on a safe non-query line while the viewport
moves through todos.

### 4. Park or repair the cursor only on safe source lines

Before or after the rendered-query scroll, verify that the cursor is not inside any query fence according to the
existing `findQueryCodeBlocks()` logic.

If the cursor is inside a query fence, move it to the nearest non-query line outside the fence:

- For downward browsing, prefer the line before the query while the query is still being viewed.
- For upward browsing, prefer the line after the query while the query is still being viewed.
- Use a cursor-setting path that avoids an extra "scroll cursor into view" when possible; if only `cm.setCursor()` is
  available, immediately restore the intended scroll position after setting the cursor.

Do not set the cursor to any rendered task's source line. Query result rows may point to tasks across the vault, and
selecting a source task line would be a different navigation action rather than a query-browsing action.

### 5. Preserve the existing fallback

If the active editor has no rendered Tasks query in or near the viewport, keep the existing behavior:

- Compute the half-page line count.
- Move the cursor by source lines.
- Skip `tasks`, `dataview`, and `dataviewjs` source fences with `findNearestNonQueryLine()`.
- Reveal the resulting cursor line.

This preserves current behavior in plain Markdown, source mode, and non-rendered query cases.

## Tests and Verification

Automated/static checks:

- `node -c plugins/task-status-cycler/main.js`
- `jq '.' plugins/task-status-cycler/manifest.json`
- `git diff --check -- plugins/task-status-cycler/main.js`

Focused helper checks:

- Query fence detection still identifies closed and unclosed `tasks`, `dataview`, and `dataviewjs` fences.
- Existing fallback target selection still skips query fences in both directions.
- Rendered-query scroll math:
  - scrolls by a bounded half viewport;
  - clamps at rendered query top/bottom;
  - does not move the cursor into a query fence;
  - falls back cleanly when no CM6 `EditorView`/DOM is available.

Manual Obsidian smoke test after `bob plugins sync -p task-status-cycler` and plugin reload:

1. Open a note with a long rendered ```tasks query.
2. Put the cursor on a safe line before the rendered query and press `<C-d>` repeatedly.
3. Confirm the viewport moves through task results gradually and does not jump straight to the bottom.
4. Confirm the source query fence remains rendered and the cursor is not placed on any line inside it.
5. Press `<C-u>` repeatedly and confirm symmetric upward scrolling.
6. At the top and bottom edges of the rendered query, confirm behavior transitions back to normal document movement
   without hiding intermediate todos.
7. Regression-check `<C-Enter>`, `<C-]>`, `o`, `O`, `<CR>`, and `<BS>` in Vim normal mode.

## Risks

- Obsidian/Tasks DOM class names can change. Mitigation: keep selectors narrow but layered, and fall back to the current
  source-line behavior when rendered query DOM is not recognized.
- Cursor parking without reveal may be sensitive to CodeMirror/Vim's trailing cursor-visibility scroll. Mitigation:
  perform the rendered-query scroll as the final operation, and restore the intended `scrollTop` after any unavoidable
  cursor repair.
- If the user presses a non-scroll command while the cursor is parked offscreen, Obsidian may reveal the cursor again.
  That is acceptable; the fix is specifically for repeated `<C-d>/<C-u>` query browsing.
