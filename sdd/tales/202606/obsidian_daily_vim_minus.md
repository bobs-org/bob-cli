---
create_time: 2026-06-04 10:04:48
status: done
prompt: sdd/prompts/202606/obsidian_daily_vim_minus.md
---
# Fix Obsidian Daily Note Minus Vim Mapping Plan

## Context

- The Bob vault is `/home/bryan/bob`, and the relevant live plugin is
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- `/home/bryan/bob/.obsidian/hotkeys.json` currently binds Obsidian's core `daily-notes` command to bare `-`. That
  hotkey is global Obsidian configuration, so it is not aware of Vim mode and can fire while editing text in insert
  mode.
- `bob-navigation-hotkeys` already registers several Vim-mode mappings through `window.CodeMirrorAdapter.Vim`, using
  `vim.mapCommand(..., { context: "normal" })` for normal-mode-only behavior.
- `/home/bryan/bob` has unrelated pre-existing dirty files. Implementation must avoid overwriting, staging, or
  committing unrelated vault changes.

## Goal

Make bare `-` open the daily note only when Obsidian Vim mode is in normal mode. In insert mode, `-` should behave like
ordinary text input and insert a hyphen.

## Non-Goals

- Do not change daily note templates, date calculation, or the core Daily Notes plugin behavior.
- Do not add any new `bob-cli` subcommands or options.
- Do not modify memory files.
- Do not rewrite unrelated Obsidian hotkeys or vault content.

## Implementation Approach

1. Move the daily-note trigger out of global hotkeys.
   - Remove the explicit bare `-` binding for `daily-notes` from `/home/bryan/bob/.obsidian/hotkeys.json`.
   - Leave unrelated hotkeys untouched.

2. Add a normal-mode Vim mapping in `bob-navigation-hotkeys`.
   - Define a new Vim action such as `bobNavigationOpenDailyNote`.
   - Register `vim.mapCommand("-", "action", "bobNavigationOpenDailyNote", {}, { context: "normal" })` alongside the
     existing `[[`, `]]`, and `!` mappings.
   - Implement the action by invoking Obsidian's existing command ID `daily-notes`, which is the same command currently
     referenced by `hotkeys.json`.
   - Capture the current editor position before opening the daily note so the plugin's alternate-file/position tracking
     stays consistent with the other navigation actions.
   - Handle missing command execution defensively with a concise Notice and console error path, rather than throwing
     from a Vim action callback.

3. Preserve existing registration behavior.
   - Keep the current `registerVimMappings()` retry flow that waits for `window.CodeMirrorAdapter.Vim`.
   - Avoid adding a global keydown listener; the mode boundary should come from CodeMirror Vim's `{ context: "normal" }`
     mapping.

## Validation

1. Static checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
   - `jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`

2. Focused Node harness:
   - Stub `window.CodeMirrorAdapter.Vim`.
   - Instantiate the plugin class enough to call `registerVimMappings()`.
   - Assert that `-` is mapped to the new action with `context: "normal"`.
   - Assert that invoking the action attempts `app.commands.executeCommandById("daily-notes")`.
   - Assert that no insert-mode `-` mapping is registered.

3. Git hygiene:
   - Check `git -C /home/bryan/bob status --short` before and after edits.
   - Confirm the intended implementation diff is limited to:
     - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
     - `/home/bryan/bob/.obsidian/hotkeys.json`

4. Manual Obsidian smoke test after reloading the plugin or Obsidian:
   - In Vim normal mode, press `-` and confirm today's daily note opens.
   - In Vim insert mode, press `-` and confirm a literal hyphen is inserted.
   - Confirm existing normal-mode mappings `[[`, `]]`, and `!` still work.

## Risks

- If Obsidian changes the core Daily Notes command ID, the Vim action would need to be updated. The current ID is taken
  from the existing working hotkey entry, so this is a low-risk dependency.
- If Vim mode is disabled or `window.CodeMirrorAdapter.Vim` is unavailable, the mapping will not register. That matches
  the requirement that the key only be active in Vim normal mode.
- Because the vault is actively synced and already dirty, final implementation must keep status/diff checks tight and
  commit only the files changed for this task as required by the vault instructions.
