---
create_time: 2026-06-15 16:12:53
status: wip
prompt: sdd/prompts/202606/revert_embed_fallback_add_ctrl_keymaps.md
---
# Revert embed fallback and replace two Vim mappings with Ctrl+Shift hotkeys

## Context

The prior change to `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` was committed as
`dcc2465 fix: route Bob Vim keys over focused embeds`. It added a capture-phase DOM keydown fallback for focused embeds,
plus helper exports and dispatch plumbing. That commit is no longer `HEAD`, so the implementation should reverse just
that commit's plugin changes, not reset the vault or disturb later commits.

The current vault has pre-existing unrelated dirty state, especially `.obsidian/hotkeys.json`. The target plugin files
and `obsidian_vimrc.md` are currently clean. Avoid taking ownership of `.obsidian/hotkeys.json`; add default hotkeys in
plugin command registration instead.

The old keymaps live in two different places:

- `\\` is registered by `bob-ledger-tools` as a CodeMirror Vim normal-mode `vim.mapCommand("\\\\", ...)`.
- `\|` is registered by `obsidian_vimrc.md` as `nmap \| :bob_dash_tasks<CR>`.

## Desired outcome

- Remove the embed keymap fallback added by `dcc2465`.
- Add `Ctrl+Shift+\` as the Obsidian hotkey for the same action currently reached by the `\\` Vim normal-mode mapping:
  jumping to the current Pomodoro line.
- Add `Ctrl+Shift+=` as the Obsidian hotkey for the same action currently reached by the `\|` Vim normal-mode mapping:
  opening `dash.md` at the `## Tasks` section.
- Remove the old `\\` and `\|` Vim normal-mode keymaps so each action has a single remembered shortcut.
- Leave unrelated vault changes untouched.

## Implementation plan

1. Re-check `~/bob` status immediately before editing and confirm the intended files are still clean or only changed by
   this task.

2. Reverse the previous embed fallback change in `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
   - Use a scoped reverse of commit `dcc2465` or an equivalent patch that removes only that commit's additions.
   - Confirm removal of `EMBED_KEYMAP_FALLBACK_*`, `matchEmbedKeymapFallback`, `registerEmbedKeymapFallback`,
     `handleEmbedFallbackKeydown`, embed source-line resolution helpers, and the helper export.

3. Add the `Ctrl+Shift+=` default hotkey to the existing `open-dash-tasks` command in `bob-navigation-hotkeys/main.js`.
   - Add `hotkeys: [{ modifiers: ["Ctrl", "Shift"], key: "=" }]` to the existing command registration.
   - Keep the command behavior unchanged.

4. Add a first-class Obsidian command for the Pomodoro jump in `~/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
   - Register a command such as `jump-to-current-pomodoro`.
   - Give it `hotkeys: [{ modifiers: ["Ctrl", "Shift"], key: "\\" }]`.
   - Implement it by resolving the active Markdown editor and calling the existing `jumpToCurrentPomodoro` path, so the
     action behavior matches the current Vim mapping without duplicating Pomodoro logic.

5. Remove the old Vim normal-mode mappings.
   - In `bob-ledger-tools/main.js`, remove the `vim.mapCommand("\\\\", "action", "bobLedgerJumpToCurrentPomodoro", ...)`
     mapping. Keep the other Pomodoro Vim mappings unchanged.
   - In `obsidian_vimrc.md`, remove `nmap \| :bob_dash_tasks<CR>`. Leave `exmap bob_dash_tasks ...` unless cleanup
     clearly requires removing it; it is not a normal-mode keymap.

6. Verify.
   - Run `node -c` on both touched plugin files.
   - Run `git diff --check` scoped to the touched files.
   - Use `rg` checks to confirm the fallback symbols and old normal-mode mappings are gone, and the new default hotkeys
     are present.
   - Manual Obsidian focus testing may still be useful afterward because terminal checks cannot prove runtime key
     dispatch over focused embed DOM.

7. Commit only the files changed for this task under `~/bob` with `/sase_git_commit`, per vault instructions.
   - Expected files: `.obsidian/plugins/bob-navigation-hotkeys/main.js`, `.obsidian/plugins/bob-ledger-tools/main.js`,
     and `obsidian_vimrc.md`.
   - Do not stage or commit pre-existing `.obsidian/hotkeys.json` changes.

## Risks and notes

- `Ctrl+Shift+\` involves a shifted backslash key on common US layouts. Use Obsidian's existing hotkey key naming
  convention (`key: "\\"` plus `Shift`) rather than encoding the produced character as `|`.
- Default plugin hotkeys are preferable here because the vault hotkeys JSON is already dirty and appears to be active
  user/sync state. If Obsidian has a user override for a command, it may supersede defaults, but the target command IDs
  currently do not have persisted entries for these two new shortcuts.
