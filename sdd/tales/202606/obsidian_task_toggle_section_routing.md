---
create_time: 2026-06-14 10:09:52
status: wip
prompt: sdd/prompts/202606/obsidian_task_toggle_section_routing.md
---
# Plan: Route Ctrl+Shift+] Task Toggles Between Tasks and Future Work

## Context

The existing `<Ctrl+Shift+]>` behavior lives in the Bob vault, not in the Rust CLI:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`

The hotkey is already registered as `task-status-cycler:toggle-obsidian-task` with `Ctrl+Shift+]`, and the plugin also
maps Vim normal mode through `taskStatusCyclerToggleObsidianTask`. The current command is line-local:

- `getObsidianTaskToggle()` decides whether to demote a proper task or promote a list item.
- `demoteObsidianTaskLine()` removes the checkbox marker, standalone `#task`, and built-in Obsidian Tasks fields while
  preserving custom inline fields and trailing block IDs.
- `promoteLineToObsidianTask()` inserts a checkbox marker if needed, adds `#task`, and adds `[created::YYYY-MM-DD]` when
  missing.
- `toggleActiveObsidianTask()` replaces only the active editor line and then adjusts the cursor column.

The vault has both sibling and nested section layouts:

```md
## Tasks

- [ ] #task Existing task

## Future Work

- Existing bullet
```

and:

```md
## Tasks

- [ ] #task Existing task

### Future Work

- Existing bullet
```

So "bottom of Tasks" must mean the bottom of the direct `Tasks` content, before a child `Future Work` subsection when
that subsection is nested under `Tasks`.

The vault currently has unrelated dirty files, but `.obsidian/plugins/task-status-cycler/main.js` is clean. This task
should only need that one plugin file; `hotkeys.json` should not change because the binding already exists.

## Goal

Keep the current conversion behavior, but when the active file contains both a `Tasks` section and a `Future Work`
section:

1. Promoting a bullet/checklist item into a proper Obsidian task moves the converted item to the bottom of `Tasks`.
2. Demoting a proper Obsidian task into a normal bullet moves the converted item to the bottom of `Future Work`.
3. If either section is missing, preserve today's in-place single-line rewrite behavior.

This is same-file routing only. It should not create missing sections, touch other notes, or alter the existing hotkey
registration.

## Behavior Specification

### Section Detection

- Detect Markdown ATX headings outside YAML frontmatter and fenced code blocks.
- Match heading titles exactly after trimming closing heading markers, with these names:
  - `Tasks`
  - `Future Work`
- Accept any heading depth (`## Tasks`, `### Future Work`, etc.).
- Use the first matching `Tasks` heading and first matching `Future Work` heading in the active document. If either is
  absent, do not route; fall back to the current in-place toggle.

### Target Insertion Points

- For `Future Work`, insert at the bottom of that section, before the next heading at the same or higher level, or EOF.
- For `Tasks`, insert at the bottom of its direct content:
  - before the next heading of any level inside the `Tasks` section;
  - otherwise before the next same-or-higher heading or EOF.
- This keeps promoted tasks out of a nested `### Future Work` subsection when the layout is `## Tasks` followed by
  `### Future Work`.
- Preserve section spacing with a simple rule: keep one blank line after a heading for the first inserted item, avoid
  adding duplicate blank lines between consecutive list items, and keep the section boundary readable.

### Item Movement

- Determine the conversion direction from the original active line:
  - proper task -> demote -> target `Future Work`;
  - anything else accepted by `getObsidianTaskToggle()` -> promote -> target `Tasks`.
- Move the active list item block, not just the first line, to avoid orphaning nested sub-bullets.
- The first line of the moved block is the converted line from the existing toggle helper.
- Child/continuation lines move with the parent and keep their relative indentation.
- The implementation should avoid changing list marker style, custom metadata, block IDs, or the existing promotion and
  demotion rules. This task is routing, not a new task-line normalization pass.
- If the active item is already in the target section, still move it to that section's bottom after conversion.
- Non-list lines remain disabled/no-op exactly as they are today.

### Cursor Behavior

- When routing is inactive, keep the current cursor-column adjustment.
- When routing moves the item, put the cursor on the moved item's converted first line at the column produced by the
  existing `getObsidianTaskToggleCursorCh()` logic, clamped to the new line length.
- Best-effort scroll the moved cursor into view, matching the plugin's existing cursor-following behavior.

## Implementation

Edit only `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.

1. Add pure section helpers near the existing line helpers and export them through `module.exports.helpers`:
   - `parseMarkdownHeadingLine(lineText)`
   - `findNamedMarkdownSection(lines, title)`
   - `findTaskRoutingSections(lines)`
   - `getSectionInsertionLine(lines, section, options)`

2. Add a pure list-item block helper:
   - `getListItemBlockRange(lines, activeLine)`
   - It should include the active line plus contiguous child/continuation lines that belong to the active list item, and
     stop before the next peer item, heading, or unrelated paragraph.

3. Add a pure routing planner:
   - `getObsidianTaskToggleDocumentPlan(lines, activeLine, cursorCh, createdDateString)`
   - It calls the existing `getObsidianTaskToggle()` for the active line.
   - If routing sections are missing, it returns an in-place replace plan equivalent to today's behavior.
   - If both sections exist, it returns delete/insert/replace data for moving the converted item block to the
     appropriate target section, with final cursor line/column.

4. Update the editor command path:
   - `getActiveObsidianTaskToggle(editor)` should gather document lines and return the document plan.
   - `toggleActiveObsidianTask()` should apply either the in-place replacement or the move plan.
   - Apply edits in a stable order so line-number shifts cannot put the block in the wrong section. A pure line-array
     final-state plan is acceptable if it preserves the document's final newline and avoids replacing the whole document
     unnecessarily.

5. Keep command registration, Vim mapping, manifest, and hotkey JSON unchanged.

## Validation

1. Syntax and diff checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob status --short` before and after, confirming unrelated dirty files remain untouched.

2. Focused Node helper checks with a mocked `obsidian` module:
   - No `Tasks`/`Future Work` sections: bullet promotion still rewrites in place.
   - Only one of the two sections exists: rewrite still happens in place.
   - Sibling `## Tasks` and `## Future Work`: promoted bullet lands at bottom of `Tasks`; demoted task lands at bottom
     of `Future Work`.
   - Nested `### Future Work` under `## Tasks`: promoted task inserts before the `### Future Work` heading; demoted
     bullet inserts at the bottom of `### Future Work`.
   - Source line above target and source line below target both route correctly.
   - Moving an item with child bullets keeps the child bullets with the converted parent.
   - Existing conversion invariants still hold: task fields are stripped on demotion, created date is added/skipped on
     promotion, block IDs remain last.
   - Cursor target follows the moved first line and uses the existing column-adjustment helper.

3. Manual smoke test after reloading Obsidian or toggling `task-status-cycler`:
   - In a scratch note with both sections, convert a Future Work bullet to a task; it appears at the bottom of `Tasks`.
   - Convert that task back; it appears at the bottom of `Future Work`.
   - Repeat in a note whose `Future Work` heading is nested under `Tasks`.
   - Confirm `<Ctrl+Shift+]>` still works from Vim normal mode.

## Risks

- Heading ambiguity: if a note has multiple `Tasks` or `Future Work` headings, the first matching heading wins. That
  keeps the behavior deterministic and matches the current simple project-note convention.
- Nested active items: the block moves with relative indentation preserved. This avoids data loss but does not try to
  rewrite arbitrary nested list structure into a top-level project task.
- Section spacing can be surprisingly personal in Markdown notes. The implementation should use small, predictable
  spacing rules and avoid broad formatting changes.
