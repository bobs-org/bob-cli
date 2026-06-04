---
create_time: 2026-06-04 17:00:35
status: wip
prompt: sdd/prompts/202606/obsidian_ta_snippet_1.md
---
# Obsidian `ta` Task Snippet Plan

## Goal

Add a Bob Obsidian snippet so typing `ta` at a snippet boundary and expanding it produces:

```text
#task <cursor> [created::2026-06-04]
```

The cursor should land between `#task ` and ` [created::2026-06-04]`, so the user can immediately type the task text.
The date should be computed from the local current date at expansion time, not hard-coded.

## Context Found

- Bryan's live Obsidian vault is `/home/bryan/bob`.
- The active snippet mechanism is the custom Obsidian plugin:
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- That plugin already registers an Obsidian command named `Expand Bob snippet` and a high-priority `Tab` keymap.
- Existing snippets are implemented through:
  - `TRIGGER_RE`
  - `parseTrigger()`
  - `computeSnippetExpansion()`
  - `findExpansion()`
  - `expandLineAtCursor()`
  - `expandFromEditor()`
- Existing snippets move the cursor to the end of the replacement. The new `ta` snippet needs custom cursor placement
  inside the replacement.
- Existing Bob task metadata commonly uses Dataview bracket fields with a space after `::`, for example
  `[created:: 2026-06-04]`. The user-requested snippet shape is explicit, so this plan will produce
  `[created::2026-06-04]` with no space after `::`.

## Implementation Plan

1. Extend trigger recognition.
   - Add `ta` to `TRIGGER_RE`.
   - Add a `task` branch in `parseTrigger()` that returns the same shape as existing snippet triggers, including
     `trigger`, `startCh`, and `endCh`.
   - Preserve the existing boundary behavior so `ta` does not expand inside words like `data` and does not expand when
     the next character is a word.

2. Add the task expansion.
   - In `computeSnippetExpansion()`, handle `trigger.kind === "task"`.
   - Use `formatLocalDate(now)` so the created date follows the plugin's existing local date formatting.
   - Return replacement text shaped as `#task  [created::YYYY-MM-DD]`.
   - Include a cursor offset pointing just after `#task `.

3. Generalize cursor placement.
   - Add a small helper that computes the target cursor column from an expansion.
   - Default to the end of the replacement for all existing snippets.
   - Use the task snippet's cursor offset only when present.
   - Update both `expandLineAtCursor()` and `expandFromEditor()` to use that helper so helper tests and live editor
     behavior match.

4. Verification.
   - Run `node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
   - Run a focused Node helper harness with lightweight stubs for Obsidian/Codemirror imports to verify:
     - `ta` expands to `#task  [created::2026-06-04]` for a fixed local date.
     - the returned cursor column is immediately after `#task `.
     - `ta` inside `data` is not recognized.
     - `ta` followed by a word character is not expanded.
     - an existing snippet such as `t0` still expands and keeps the cursor at the end.

## Files To Change

- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`

## Files Not Planned For Change

- No vault notes.
- No Obsidian CSS snippets.
- No QuickAdd or Templater settings.
- No `bob-cli` Rust code.
