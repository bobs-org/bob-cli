---
create_time: 2026-06-14 11:51:38
status: proposed
prompt: sdd/prompts/202606/obsidian_alt_bracket_bullet_formatting.md
---

# Plan: Alt-Bracket Bullet Formatting Cycle

## Context

The requested keymaps already exist in Bryan's live Obsidian vault, not in the `bob-cli` Rust CLI:

- `Alt+]` -> `task-status-cycler:cycle-task-status-forward`
- `Alt+[` -> `task-status-cycler:cycle-task-status-backward`
- Implementation: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- Bindings: `/home/bryan/bob/.obsidian/hotkeys.json`

Relevant context reviewed:

- Required Obsidian memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian workflow context before planning changes to Obsidian keymap behavior"`.
- Workspace short memory: `memory/short/sase.md`.
- Live vault instructions: `/home/bryan/bob/AGENTS.md`.
- Existing task-toggle plans, especially the checkbox and Obsidian-task toggle work.
- Current live plugin and hotkey JSON.

Important current behavior:

- `Alt+]` / `Alt+[` call `handleCycleCommand(..., direction)`.
- That command currently enables only when the active line has a Markdown checkbox marker.
- For `#task` lines, it prefers the Obsidian Tasks plugin command path and falls back to local metadata-aware rewrites.
- For non-`#task` checklist lines, it locally cycles the checkbox status through `[" ", "/", "B", "x", "-"]`.
- Plain bullets are currently ignored because `getActiveTaskStatus()` returns `null`.

The live vault is dirty. The relevant plugin file is already modified before this task; those changes include recent
task routing and dependency-ID normalization work. The implementation must build on that file as-is and avoid reverting
or rewriting unrelated dirty vault files. `hotkeys.json` does not need to change because the requested bindings already
exist.

## Goal

Keep `Alt+]` / `Alt+[` unchanged on checkbox task lines, but when the active line is a normal non-checkbox list item,
cycle the whole visible bullet body through four whole-bullet formatting states:

Forward, `Alt+]`:

```md
- Foo
- **Foo**
- _Foo_
- ~~Foo~~
- Foo
```

Backward, `Alt+[`:

```md
- Foo
- ~~Foo~~
- _Foo_
- **Foo**
- Foo
```

This is a line-local formatting cycle. It should format the current list item's body after the list marker, not the list
marker itself, and it should not affect task/checklist lines.

## Behavior Specification

1. Task lines keep existing behavior.
   - If the active line matches the existing task status parser (`- [ ]`, `- [x]`, `- [/]`, etc.), `Alt+]` and `Alt+[`
     continue to cycle task status exactly as they do today.
   - This applies to both proper `#task` lines and non-`#task` checklist lines.

2. Plain list item fallback.
   - If no checkbox status is present, try to parse the active line with the existing list-item parser
     (`LIST_ITEM_MARKER_RE`).
   - Supported plain list items should follow the plugin's existing list parsing conventions: indentation, optional
     blockquote prefixes, unordered markers, and ordered markers where the shared regex already supports them.
   - Return `false` / no-op for non-list lines.
   - Return `false` / no-op for an empty bullet body, because producing empty Markdown wrappers like `****` is not
     useful.

3. Formatting state model.
   - Treat these as whole-bullet states: `normal`, `bold`, `italic`, `strike`.
   - Detect whole-body wrappers only when the body is fully wrapped, ignoring leading/trailing whitespace around the
     body text.
   - Recognize canonical wrappers:
     - bold: `**text**`
     - italic: `*text*`
     - strike: `~~text~~`
   - Optionally accept common alternatives for detection (`__text__`, `_text_`) but rewrite using the canonical markers
     above so repeated cycling normalizes the line.
   - Partial inline formatting inside an otherwise unwrapped bullet does not define the whole-bullet state. It is
     preserved as content, and the line is treated as `normal` for the cycle.

4. Preserve structural Markdown.
   - The list marker and spacing remain untouched.
   - If the line ends in a valid Obsidian block id (`^some-id`), keep that block id outside the formatting wrapper:
     `- Foo ^id` -> `- **Foo** ^id`.
   - This preserves Obsidian block-link behavior while still formatting the visible bullet text.
   - Inline links and ordinary inline text remain inside the wrapper.

5. Cursor behavior.
   - Reuse the existing `getCursorChAfterTextEdits()` style of cursor adjustment.
   - Keep the cursor near the same visible content after wrappers are inserted or removed.
   - Clamp the final cursor column to the rewritten line length.

6. Selection behavior.
   - Match the current task-status commands: operate on the active cursor line.
   - Do not introduce multi-line selection behavior in this change.
   - Do not try to wrap nested child bullets or continuation blocks, because multi-line Markdown emphasis around nested
     list blocks is ambiguous and easier to corrupt. Nested bullets can be formatted by placing the cursor on each child
     item.

## Implementation

Edit only:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No expected edits:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json`
- `bob-cli` Rust code, scripts, tests, or docs
- Memory files

Implementation steps:

1. Re-check live vault state immediately before editing.
   - Inspect `git -C /home/bryan/bob status --short`.
   - Inspect the targeted diff for `.obsidian/plugins/task-status-cycler/main.js`.
   - Preserve the existing dirty plugin changes and add only the bullet-formatting delta.

2. Add small pure helpers near the existing task/list rewrite helpers.
   - `getPlainListItemFormattingTarget(lineText)`:
     - returns prefix/body/body range for non-checkbox list items;
     - returns `null` for checkbox lines, non-list lines, and empty visible body.
   - `splitTrailingBlockIdFromBody(bodyText)`:
     - separates visible content from a trailing `^block-id` suffix, preserving spacing.
   - `getWholeBulletFormatState(bodyText)`:
     - detects `normal`, `bold`, `italic`, or `strike` for the visible body.
   - `getAdjacentBulletFormatState(currentState, direction)`:
     - cycles through `["normal", "bold", "italic", "strike"]`.
   - `getPlainBulletFormatToggle(lineText, direction)`:
     - returns `{ sourceLineText, lineText, edits }` or `null`.

3. Export the pure helpers through `module.exports.helpers`.
   - This follows the plugin's existing testability pattern and allows a Node harness to test behavior without launching
     Obsidian.

4. Add editor-level active-line methods.
   - `getActivePlainBulletFormatToggle(editor, direction)`:
     - reads the current cursor line;
     - calls `getPlainBulletFormatToggle()`;
     - returns line/cursor metadata.
   - `toggleActivePlainBulletFormat(editor, toggle)`:
     - replaces only the active line;
     - restores the cursor via the edit-aware cursor helper.

5. Extend `handleCycleCommand(checking, editor, view, direction)`.
   - Keep the current task-status path first.
   - If a task status exists and has a next symbol, behave exactly as today.
   - If no task status exists, try the plain-bullet formatting fallback.
   - In check mode, return true when either task cycling or bullet-format cycling is available.
   - In apply mode, execute the selected path and return true.
   - If neither path applies, return false.

6. Leave hotkeys and command IDs unchanged.
   - Existing `Alt+]` / `Alt+[` bindings automatically inherit the new fallback because they already dispatch to the
     cycle commands.
   - Do not add new Obsidian commands unless implementation reveals a concrete need.
   - Do not alter the existing Vim mappings. The current user-visible keymaps are Obsidian hotkeys; changing the command
     handler is the smallest surface.

## Validation

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob status --short -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json
```

Focused Node helper assertions with a mocked `obsidian` module:

- `- Foo`, forward -> `- **Foo**`
- `- **Foo**`, forward -> `- *Foo*`
- `- *Foo*`, forward -> `- ~~Foo~~`
- `- ~~Foo~~`, forward -> `- Foo`
- `- Foo`, backward -> `- ~~Foo~~`
- `- ~~Foo~~`, backward -> `- *Foo*`
- `- *Foo*`, backward -> `- **Foo**`
- `- **Foo**`, backward -> `- Foo`
- `  - Foo` preserves indentation and marker.
- `> - Foo` preserves blockquote prefix.
- `1. Foo` behaves consistently with the shared list-item parser if supported by `LIST_ITEM_MARKER_RE`.
- `- Foo ^abc123` -> `- **Foo** ^abc123`, and cycling back preserves the block id.
- `- Foo **bar**` is treated as normal whole-bullet state and preserves the inner formatting as content.
- `- [ ] #task Foo` does not use the bullet-format helper; existing status cycling still applies.
- `- [x] Foo` does not use the bullet-format helper; existing checkbox status cycling still applies.
- Non-list text returns `null`.
- Empty bullets return `null`.
- Cursor adjustment is stable when wrappers are added and removed.

Manual smoke test after reloading Obsidian or toggling `task-status-cycler`:

1. On `- Foo`, press `Alt+]` repeatedly and confirm the cycle is bold -> italic -> strike -> normal.
2. On `- Foo`, press `Alt+[` and confirm it goes to strike.
3. On a nested bullet, confirm the marker/indentation is preserved.
4. On `- Foo ^id`, confirm `^id` remains a working block id after formatting.
5. On a real `#task` checkbox line, confirm `Alt+]` / `Alt+[` still cycle task status and Tasks metadata behavior is
   unchanged.

Final hygiene after implementation edits:

- Review `git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js`.
- Confirm no unrelated vault files were touched.
- Because `/home/bryan/bob/AGENTS.md` requires committing after vault file edits, commit only the task-related plugin
  file using the SASE git commit workflow before finishing implementation.

## Risks

- The plugin file already contains uncommitted changes. The implementation must be incremental and should avoid broad
  formatting or moving helper blocks unnecessarily.
- Markdown emphasis around nested list blocks is ambiguous. This plan intentionally keeps the operation line-local.
- Whole-line wrapping can interact with existing inline formatting. The helper should preserve inner formatting text,
  but only whole-body wrappers define the cycle state.
- Block IDs must remain outside the wrapper; otherwise Obsidian block links can break.
