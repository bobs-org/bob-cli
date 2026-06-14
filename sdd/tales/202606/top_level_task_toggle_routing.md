---
create_time: 2026-06-14 10:29:40
status: proposed
prompt: sdd/prompts/202606/top_level_task_toggle_routing.md
---

# Plan: Restrict Task Toggle Section Routing to Top-Level Dash Items

## Context

The existing `<Ctrl+Shift+]>` command is implemented in the Bob Obsidian vault plugin:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

The recent routing change made `getObsidianTaskToggleDocumentPlan()` move converted items between the first `Tasks`
section and the first `Future Work` section when both sections exist:

- Bullet/checklist item -> proper Obsidian task: move to the bottom of `Tasks`.
- Proper Obsidian task -> normal bullet: move to the bottom of `Future Work`.
- If either section is missing: keep the older in-place line rewrite.

The current planner uses `getObsidianTaskToggle()` to decide whether the active line can be converted. That helper
accepts indented list items, blockquoted list items, alternate bullet markers, and ordered-list markers because its list
regexes are intentionally broad:

- `  - nested item`
- `\t- nested item`
- `> - quoted item`
- `* alternate marker`
- `1. ordered item`

The new requirement is narrower: section routing should only apply when the active item is a top-level dash bullet/task,
defined as having no whitespace before the leading `-` character. Indented bullets/tasks should not be routed.

## Goal

Keep the existing conversion command available everywhere it currently works, but restrict the cross-section move
behavior to source lines whose list marker begins at column 0 with `-`.

Assumption for this implementation: the phrase "leading `-` character" is strict. A top-level `*`, `+`, ordered list
item, or blockquoted item will still convert in place, but it will not move between `Tasks` and `Future Work`.

## Behavior Specification

### Routing Eligibility

Section routing is active only when all of these are true:

1. `getObsidianTaskToggle(sourceLineText, createdDateString)` returns a conversion rewrite.
2. The active document contains both the selected `Tasks` and `Future Work` sections.
3. The original source line starts with a dash list marker at column 0:
   - `- Plain bullet`
   - `- [ ] Checklist item`
   - `- [ ] #task Proper task`
   - `-\tTabbed-after-marker item`

These lines should not route, even when both sections exist:

- `  - Indented bullet`
- `\t- Tab-indented bullet`
- `  - [ ] #task Indented task`
- `> - Blockquoted bullet`
- `* Alternate bullet marker`
- `+ Alternate bullet marker`
- `1. Ordered list item`

### Conversion Fallback

When the source line is not routing-eligible, the command should return the same in-place replace plan that it already
uses when a document is missing either routing section. This preserves current conversion support for nested bullets and
tasks without moving them out of their local context.

### Block Movement

When the source line is routing-eligible, keep the existing block movement behavior:

- Move the converted first line plus more-indented child/continuation lines.
- Preserve child indentation and relative order.
- Apply the existing Tasks/Future Work insertion rules.
- Preserve the existing cursor-following behavior.

If the active line is an indented child item, only that child line should be converted in place. It should not drag its
children to another section, and it should not move the parent item.

## Implementation

Edit only:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

Implementation steps:

1. Add a small pure predicate near the existing line helper functions, for example
   `isTopLevelDashListToggleLine(lineText)`.
   - It should return true only when `String(lineText || "")` begins with `-[ \t]+`.
   - It should not decide whether a line can be converted; `getObsidianTaskToggle()` already owns conversion
     eligibility.
   - Export it through `module.exports.helpers` so the rule can be tested directly.

2. Update `getObsidianTaskToggleDocumentPlan()` after it builds the existing `inPlacePlan`.
   - Keep the current early `null` behavior for invalid lines or non-convertible lines.
   - Find routing sections as it does today.
   - Return `inPlacePlan` if either section is missing.
   - Return `inPlacePlan` if `isTopLevelDashListToggleLine(sourceLineText)` is false.
   - Only call `getListItemBlockRange()` and build a move plan after the top-level dash eligibility check passes.

3. Keep the editor application path unchanged.
   - `toggleActiveObsidianTask()`, `applyObsidianTaskReplacePlan()`, and `applyObsidianTaskMovePlan()` should not need
     behavioral changes.
   - Command registration, Vim mappings, manifest, and hotkeys should remain unchanged.

## Validation

1. Static checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob status --short -- .obsidian/plugins/task-status-cycler/main.js`

2. Focused Node helper assertions with a mocked `obsidian` module:
   - A top-level `- Future item` in `Future Work` promotes and moves to `Tasks`.
   - A top-level `- [ ] #task Current task` in `Tasks` demotes and moves to `Future Work`.
   - An indented `  - Future item` promotes in place even when both sections exist.
   - An indented `  - [ ] #task Current task` demotes in place even when both sections exist.
   - A tab-indented `\t- Future item` promotes in place.
   - A top-level `* Future item`, `+ Future item`, or `1. Future item` promotes in place.
   - A blockquoted `> - Future item` promotes in place.
   - A top-level dash item with child bullets still routes as a block and carries its children.
   - Missing one or both sections still uses in-place conversion for both top-level and indented items.
   - Cursor line/column behavior remains unchanged for both move and replace plans.

3. Manual smoke test after reloading Obsidian or toggling the plugin:
   - In a scratch note with both sections, convert a top-level `-` Future Work bullet; it moves to `Tasks`.
   - Convert that top-level task back; it moves to `Future Work`.
   - Convert an indented child bullet/task under either section; it changes line-local syntax but stays in place.
   - Repeat in a note whose `Future Work` heading is nested under `Tasks`.
   - Confirm `<Ctrl+Shift+]>` still works from Vim normal mode.

## Risks

- The strict dash-only interpretation means top-level ordered-list items and `*`/`+` bullets will no longer route. That
  matches the requested "leading `-`" wording, but it is intentionally narrower than the previous routing behavior.
- A simple `^-[ \t]+` predicate delegates detailed syntax validation to the existing conversion helper. This keeps the
  new rule small and avoids creating a second source of truth for task/bullet parsing.
- The plugin file already contains uncommitted routing changes from the prior implementation. This change should be a
  small incremental diff on top of that work, not a rewrite.
