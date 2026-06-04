---
create_time: 2026-06-04 14:26:50
status: done
prompt: sdd/prompts/202606/obsidian_vim_o_list_continuation.md
---
# Obsidian Vim `o` List Continuation Plan

## Context

- The requested behavior is for the live Obsidian vault at `/home/bryan/bob`, not for the `bob-cli` Rust command
  surface.
- Project memory was reviewed through `sase memory read long/obsidian.md`, as required for Obsidian work.
- `/home/bryan/bob/AGENTS.md` requires checking vault status before editing, preserving unrelated dirty/synced files,
  and committing any task-related vault changes before finishing.
- The relevant implementation target is `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`. This plugin
  already owns task-aware Vim normal-mode behavior:
  - `<CR>` toggles open/done tasks.
  - `<C-d>` and `<C-u>` provide custom normal-mode movement.
  - Vim mappings are registered through `window.CodeMirrorAdapter.Vim` with `{ context: "normal" }`.
- There is no existing custom `o` Vim mapping.
- `/home/bryan/bob` already has unrelated dirty note files. The current target plugin file is clean before this task.

## Goal

When pressing `o` in Obsidian Vim normal mode:

- If the current line is an indented checkbox/task line, insert a new line below with the same indentation and `- [ ] `,
  then enter insert mode with the cursor after that prefix.
- If the current line is an indented bullet line without a checkbox, insert a new line below with the same indentation
  and `- `, then enter insert mode with the cursor after that prefix.
- For other lines, preserve the expected basic `o` behavior: create a line below and enter insert mode.

Examples:

```text
- [x] done
+- [ ] |
```

```text
  - topic
  - |
```

## Non-Goals

- Do not add or change `bob-cli` subcommands/options.
- Do not modify memory files.
- Do not change global Obsidian hotkeys.
- Do not rewrite unrelated plugin behavior, manifests, styles, or synced note content.
- Do not try to reimplement broader Markdown auto-continuation semantics such as ordered lists, nested blockquote
  syntax, or task status inheritance unless needed by the requested bullet/checkbox behavior.

## Implementation Approach

1. Extend `task-status-cycler` Vim mappings.
   - Define a new Vim action such as `taskStatusCyclerOpenLineBelow`.
   - Register `vim.mapCommand("o", "action", "taskStatusCyclerOpenLineBelow", {}, { context: "normal" })`.
   - Keep the existing `registerVimMappings()` retry flow unchanged.

2. Add small, focused line-prefix helpers.
   - Parse the current line's leading whitespace indentation.
   - Detect checkbox/task bullet lines with a regex that accepts common Markdown bullet markers followed by `[status]`,
     while always creating a new unchecked task prefix: `${indent}- [ ] `.
   - Detect plain bullet lines without a checkbox and create `${indent}- `.
   - Prefer the requested literal dash prefix for continuation rather than preserving `*` or `+` markers.

3. Implement the `o` action through CodeMirror editor APIs.
   - Read the current cursor and current line from the Vim callback's CodeMirror object.
   - Insert `"\n" + continuationPrefix` at the end of the current line using `replaceRange`.
   - Place the cursor on the new line after the inserted prefix.
   - Enter Vim insert mode with `window.CodeMirrorAdapter.Vim.handleKey(cm, "i", "mapping")`, matching the existing
     pattern used by installed Obsidian plugins.
   - If the current line is neither a checkbox nor a bullet, insert a basic indented blank line below and enter insert
     mode so `o` remains usable.

4. Keep testability local.
   - Export pure helper functions from `task-status-cycler/main.js` in the same style used by other local plugins'
     helper exports, without changing Obsidian runtime behavior.
   - Use a lightweight Node harness with a stubbed `obsidian` module and fake CodeMirror object to assert line-prefix
     behavior and mapping registration.

## Validation

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused behavior checks:

```bash
node <inline harness>
```

The harness should verify:

- `  - [x] done` opens ` - [ ]` below and puts the cursor after the prefix.
- `  - [ ] open` opens ` - [ ]` below.
- `  - topic` opens ` -` below.
- A non-list indented line still opens a new indented line below.
- `registerVimMappings()` maps `o` only with `{ context: "normal" }`.

Git hygiene:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js
git status --short
```

If a vault file is changed, commit only `.obsidian/plugins/task-status-cycler/main.js` with the required
`sase_git_commit` workflow before finishing.

Manual live-vault smoke test after reloading the plugin or Obsidian:

- In Vim normal mode on an indented task line, press `o`; confirm the new line has the same indentation plus `- [ ] `
  and insert mode starts after the prefix.
- In Vim normal mode on an indented bullet line, press `o`; confirm the new line has the same indentation plus `- ` and
  insert mode starts after the prefix.
- In Vim normal mode on a non-list line, press `o`; confirm a new line below opens in insert mode.
- Confirm existing `task-status-cycler` mappings still work: `<CR>`, `<C-d>`, and `<C-u>`.

## Risks

- Overriding Vim's built-in `o` means the plugin owns the basic open-line-below behavior while it is enabled. The
  implementation should keep the fallback simple and predictable.
- CodeMirror/Obsidian Vim internals are not a public API. The insert-mode transition should use the same
  `Vim.handleKey(..., "i", "mapping")` shape already used by installed Obsidian plugins.
- Live Obsidian confirmation may still be needed after static and harness checks because Vim-mode state transitions are
  ultimately runtime behavior.
