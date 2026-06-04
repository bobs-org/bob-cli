---
create_time: 2026-06-03 23:04:51
status: wip
prompt: sdd/prompts/202606/daily_dash_keymap.md
---
# Daily Dash Keymap Plan

## Goal

Move the Obsidian daily-note jump from `<Alt+->` to bare `-` while preserving the existing Bob navigation bindings and
avoiding accidental edits to unrelated synced vault files.

## Current Findings

- The existing daily jump is not in `bob-ledger-tools` or the Pomodoro Vim mappings. It is the core Obsidian
  `daily-notes` command configured in `~/bob/.obsidian/hotkeys.json`:
  - current: `daily-notes` -> modifiers `["Alt"]`, key `"-"`.
- `~/bob/.obsidian/hotkeys.json` has no existing bare `-` assignment.
- `Ctrl+-` is already assigned to `bob-navigation-hotkeys:open-child-note`; this should remain unchanged.
- `bob-navigation-hotkeys` registers custom commands and a few Vim normal-mode mappings, but it does not currently own
  the daily-note command.
- The main risk is that a bare `-` Obsidian hotkey may be too global. It could conflict with ordinary hyphen typing or
  with Vim normal-mode's built-in `-` motion depending on how Obsidian dispatches unmodified hotkeys in the active
  editor.

## Implementation Approach

1. Inspect the vault status immediately before editing.
   - Keep the already-dirty synced note files untouched.
   - Only stage/commit files changed for this task.

2. Make the smallest literal configuration change first.
   - Edit `~/bob/.obsidian/hotkeys.json`.
   - Change only the `daily-notes` binding from:
     - modifiers `["Alt"]`, key `"-"`
   - to:
     - modifiers `[]`, key `"-"`
   - Do not change `Ctrl+-`, plugin code, manifests, or other hotkeys.

3. Treat plugin-code changes as a fallback, not the default.
   - If validation or manual testing shows that the bare Obsidian hotkey hijacks normal text entry, revert the
     `hotkeys.json` key change and instead implement a Vim-normal-mode-only mapping in `bob-navigation-hotkeys`.
   - That fallback would add a dedicated action/command path that runs Obsidian's existing `daily-notes` command, then
     map Vim normal-mode `-` to it.
   - This fallback preserves typed hyphens in insert/edit contexts but would intentionally replace Vim normal-mode's
     built-in `-` behavior.

## Validation

1. Static checks:
   - Run `jq '.' ~/bob/.obsidian/hotkeys.json`.
   - Confirm the only intended hotkey diff is the `daily-notes` modifier array changing from `["Alt"]` to `[]`.
   - Search the hotkey config for duplicate bare `-` bindings.

2. Behavior checks:
   - In Obsidian, press `-` from a normal note/navigation context and confirm today's daily note opens.
   - Confirm `Alt+-` no longer opens the daily note unless Obsidian still has another default binding.
   - Confirm `Ctrl+-` still opens the child-note picker.
   - Confirm typing `-` in a normal editable text context still works. If it does not, use the fallback plugin/Vim-only
     plan above.

3. Repository hygiene:
   - Check `git -C ~/bob status --short` before and after the edit.
   - Commit only `~/bob/.obsidian/hotkeys.json` with `sase_git_commit`, per the vault `AGENTS.md`.
   - Leave unrelated dirty notes and untracked synced files untouched.

## Expected Outcome

The preferred outcome is a one-file vault configuration commit where the core `daily-notes` hotkey becomes bare `-`. If
Obsidian's global hotkey handling makes that unsafe for text entry, the implementation should pivot to a plugin-owned
Vim-normal-mode mapping instead of shipping a disruptive global bare-key hotkey.
