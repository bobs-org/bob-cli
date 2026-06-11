---
create_time: 2026-06-11 10:26:08
status: done
prompt: sdd/prompts/202606/obsidian_ctrl_shift_bracket_task_toggle.md
---
# Plan: Obsidian Ctrl+Shift+] Proper-Task Toggle

## Context

Bryan's Obsidian vault is `~/bob`. The local plugin `~/bob/.obsidian/plugins/task-status-cycler/main.js` already owns
custom task-line behavior, including the existing `<Ctrl+]>` keymap (`toggle-task-checkbox-marker`) that toggles a line
between a checkbox marker and a plain list bullet. That command is wired up three ways:

1. An Obsidian editor command registered with `editorCheckCallback`.
2. A CodeMirror Vim normal-mode mapping registered through `window.CodeMirrorAdapter.Vim.mapCommand`.
3. A binding in `~/bob/.obsidian/hotkeys.json`.

Relevant facts gathered from the vault:

- The Tasks plugin (`obsidian-tasks-plugin`) is configured with `taskFormat: "dataview"` and `globalFilter: "#task"`. A
  "proper Obsidian task" is therefore a checkbox line whose text contains the `#task` tag.
- Real task lines look like: `- [ ] #task Improve the columns in [[refs.base]]! [p::1] [created::2026-06-05]`
  `- [x] #task Add PDF task support! [p::1] [created::2026-06-05]  [completion:: 2026-06-08] ^tasks-in-pdfs`
- Recent `created` fields in the vault use the compact `[created::YYYY-MM-DD]` form (no space after `::`), matching the
  format requested for this feature.
- The plugin already has pure helpers for: checkbox-marker toggling (`getTaskCheckboxMarkerToggle`), `#task` global
  filter detection (`lineMatchesTasksGlobalFilterText`), local-date formatting (`formatLocalDate`), completion-field
  removal with trailing block-ID spacing normalization (`removeCompletionField`, `normalizeTaskMetadataSpacing`), and
  cursor repositioning (`getTaskCheckboxMarkerCursorCh`).
- `hotkeys.json` has no `Ctrl+Shift+]` binding today (no conflict to resolve), and no `~/bob/.obsidian.vimrc` exists.
- No `#task/<subtag>` usages exist in the vault, so plain `#task` tag handling is sufficient.
- The vault is actively synced and has pre-existing uncommitted changes; per `~/bob/AGENTS.md`, only the files changed
  for this task may be staged/committed, via `/sase_git_commit`.

## Goal

Add a `<Ctrl+Shift+]>` keymap that toggles whether the active bullet/checkbox line is a proper Obsidian task.

Demote — pressing it on a proper task:

```md
- [ ] #task Foo bar baz [p::1] [created::2026-06-05] [completion:: 2026-06-08] ^abc
```

becomes (checkbox marker removed, `#task` tag removed, Tasks properties removed; custom fields and block ID kept):

```md
- Foo bar baz [p::1] ^abc
```

Promote — pressing it on a non-task bullet or checkbox:

```md
- Foo bar baz
```

becomes (using today's date):

```md
- [ ] #task Foo bar baz [created::2026-06-11]
```

## Behavior Specification

1. Proper-task detection: the line has a checkbox marker (existing `TASK_CHECKBOX_MARKER_RE`) AND matches the Tasks
   global filter (`lineMatchesTasksGlobalFilterText`, i.e. contains a standalone `#task` tag).

2. Demote (proper task -> plain bullet):
   - Remove every standalone `#task` tag occurrence, consuming adjacent separator whitespace so no double spaces are
     left behind.
   - Remove dataview-format Tasks properties from a fixed key list: `created`, `completion`, `due`, `scheduled`,
     `start`, `cancelled`, `priority`, `repeat`, `id`, `dependsOn`, `onCompletion` — matching both `[key:: value]` and
     `[key::value]` spacing (generalizing the existing `COMPLETION_FIELD_RE` approach).
   - Deliberately preserve: indentation, blockquote prefixes, ordered/unordered list markers, Bryan's custom inline
     fields (e.g. `[p::1]`, `[h::...]`, `[snooze::...]`), and any trailing `^block-id` (with spacing normalized via
     `normalizeTaskMetadataSpacing`). The property key list is a single constant, easy to extend later if custom fields
     should also be stripped.
   - Remove the checkbox marker, converting to a plain bullet (same rewrite as the existing `<Ctrl+]>` behavior).

3. Promote (anything else that is a list item -> proper task):
   - If the line has no checkbox marker, insert `[ ] ` after the list marker (existing toggle helper logic). If it is
     already a checkbox, keep its current status symbol (e.g. `[x]`, `[/]`) untouched.
   - Prepend `#task ` to the body (immediately after the checkbox marker) — skipped if the line already matches the
     global filter, so half-states never get a duplicate tag.
   - Append ` [created::YYYY-MM-DD]` using today's local date (`formatLocalDate`), with a single leading space and no
     space after `::` (matching recent vault convention and the requested format). The field is inserted before any
     trailing `^block-id`, and skipped if a `created` field is already present.

4. The command is disabled (check callback returns false) on lines that are not list items at all; the Vim action is a
   no-op there.

5. Cursor handling: best-effort — shift the cursor column by the net length delta of edits occurring at or before the
   cursor position, then clamp to the rewritten line length (same philosophy as `getTaskCheckboxMarkerCursorCh`).

## Implementation

1. Extend `task-status-cycler/main.js` (keeps all task/list editing in the one local plugin; no manifest changes
   needed).

2. Add pure line-rewrite helpers (exported via `module.exports.helpers` for testability):
   - `isProperObsidianTaskLine(lineText)` — checkbox marker + global filter match.
   - `demoteObsidianTaskLine(lineText)` — tag/property removal + marker removal, per the spec above.
   - `promoteLineToObsidianTask(lineText, createdDateString)` — marker insertion + `#task ` prefix + created field.
   - `getObsidianTaskToggle(lineText, createdDateString)` — dispatches to demote/promote, returns `null` for non-list
     lines, and carries the cursor-delta info needed for cursor repositioning.

3. Add an Obsidian editor command:
   - Command id: `toggle-obsidian-task`; name: `Toggle Obsidian task`.
   - `editorCheckCallback` enabled only when `getObsidianTaskToggle` returns a rewrite for the active line.
   - Replaces only the active line and repositions the cursor per the spec.

4. Add a CodeMirror Vim normal-mode mapping:
   - Define action `taskStatusCyclerToggleObsidianTask`, reusing the same toggle path as the command (mirroring how
     `taskStatusCyclerToggleCheckboxMarker` works).
   - Map it next to the existing `<C-]>` mapping. Since Shift+`]` produces `}`, the keystroke reaches CodeMirror Vim as
     `<C-}>`; map `<C-}>` (and `<C-S-]>` defensively — an unused mapping is harmless).

5. Add the vault hotkey:
   - Add `task-status-cycler:toggle-obsidian-task` with `{ "modifiers": ["Ctrl", "Shift"], "key": "]" }` to
     `~/bob/.obsidian/hotkeys.json`. No existing binding uses `Ctrl+Shift+]`, so nothing needs to move.

## Validation

1. Syntax/config checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json`

2. Node helper test with a mocked `obsidian` module covering at least:
   - `- [ ] #task Foo [created::2026-06-10]` -> `- Foo`
   - `- [x] #task Foo [p::1] [created::2026-06-05]  [completion:: 2026-06-08] ^id` -> `- Foo [p::1] ^id`
   - `- Foo` -> `- [ ] #task Foo [created::<today>]`
   - `- [x] Foo` -> `- [x] #task Foo [created::<today>]`
   - `- Foo ^id` -> `- [ ] #task Foo [created::<today>] ^id`
   - `  3) #task indented ordered task [created::2026-06-01]` round-trips to an ordered non-task item and back.
   - Non-list text returns `null` (command disabled).
   - Promote-then-demote on a fresh bullet returns the original bullet (round-trip stability).

3. Review the final vault diff to confirm only the plugin `main.js` and `hotkeys.json` changed, leaving the vault's
   pre-existing synced changes untouched.

4. Commit only those two vault files via `/sase_git_commit` (required by `~/bob/AGENTS.md`).

## Manual Smoke Test

After reloading Obsidian (or toggling the `task-status-cycler` plugin):

1. On `- Foo bar baz`, press `<Ctrl+Shift+]>`; confirm it becomes `- [ ] #task Foo bar baz [created::2026-06-11]`
   (today's date).
2. Press `<Ctrl+Shift+]>` again; confirm it returns to `- Foo bar baz`.
3. On a real done task with `[completion:: ...]` and a block ID, demote it and confirm the custom `[p::N]` field and
   `^block-id` survive while `#task` and the Tasks properties disappear.
4. Repeat in Vim normal mode to confirm the CodeMirror Vim mapping also works.
