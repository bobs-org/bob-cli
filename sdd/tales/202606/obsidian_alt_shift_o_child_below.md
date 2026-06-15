---
title: Repurpose Alt+Shift+o to Open Child Bullet Below
create_time: 2026-06-15 08:07:13
status: done
prompt: sdd/prompts/202606/obsidian_alt_shift_o_child_below.md
---

# Repurpose Alt+Shift+o to Open Child Bullet Below

## Goal

Stop spending effort on plain `Alt+o` / `Option+o` delivery. Use the already-working `Alt+Shift+o` / `Option+Shift+o`
chord as the practical keymap for the desired behavior:

- In Vim normal mode, `Alt+Shift+o` creates a child bullet below the current line.
- The generated line uses the existing child-bullet prefix rule: current leading whitespace plus one Obsidian tab indent
  plus `- `.
- Cursor lands after `- ` and Vim enters insert mode.
- The previous `Alt+Shift+o` "child bullet above" behavior is intentionally discarded.

Plain `Alt+o` may remain configured or present internally during the transition, but it is no longer the acceptance
target for this change.

## Context Reviewed

- Required Obsidian memory was read with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and plugin workflow context before planning a keymap behavior change"`.
- Workspace short memory `memory/short/sase.md` was reviewed.
- Prior approved plan `sdd/tales/202606/obsidian_alt_o_below_keymap_fix_1.md` is now `status: done`.
- Live vault status shows only unrelated dirty/untracked Markdown files; the task plugin and hotkeys file are currently
  clean.
- Live target plugin: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- Live hotkey config: `/home/bryan/bob/.obsidian/hotkeys.json`
- Current hotkeys:
  - `task-status-cycler:open-child-bullet-line-below` -> `Alt` + `o`
  - `task-status-cycler:open-child-bullet-line-above` -> `Alt` + `O`

## Diagnosis

The new user observation is decisive: after the previous fix, plain `Alt+o` still does not work in the GUI, while
`Alt+Shift+o` is usable enough that repurposing it is attractive.

There are two active paths that can handle `Alt+Shift+o`:

1. Obsidian's native hotkey dispatch can invoke the command currently bound to `Alt+O` (`open-child-bullet-line-above`).
2. The plugin's capture-phase fallback currently maps shifted `KeyO` / `Ø` / beforeinput `Ø` to direction `"above"`.

Because that fallback runs early, a hotkey-only edit would be incomplete: even if `hotkeys.json` binds `Alt+Shift+o` to
the below command, the plugin fallback may intercept the same chord first and still insert above. The fix should make
both paths agree that the working shifted chord means "below."

## Implementation Strategy

Make `Alt+Shift+o` the supported child-bullet-below keymap at both layers:

1. Update `.obsidian/hotkeys.json` so the below command is bound to `Alt+O`.
   - Change `task-status-cycler:open-child-bullet-line-below` from `Alt+o` to `Alt+O`.
   - Remove the `Alt+O` binding from `task-status-cycler:open-child-bullet-line-above`, or leave that command with an
     empty binding list if preserving the key is clearer for Obsidian's config format.
   - Do not introduce a replacement keybinding for "above"; that behavior is no longer needed.

2. Update `task-status-cycler/main.js` so the custom fallback also maps the shifted chord to below.
   - In `getChildBulletKeydownDirection`, make `Alt+KeyO` / `Alt+Shift+KeyO` / `ø` / `Ø` return `"below"`.
   - In `getChildBulletBeforeInputDirection`, make both `data === "ø"` and `data === "Ø"` return `"below"`.
   - Keep ctrl/meta rejection, editor-target gating, and Vim normal-mode gating unchanged.
   - Keep the existing below insertion handler unchanged.

3. Optionally reduce confusing command naming without breaking hotkeys.
   - Keep command IDs stable to avoid surprising Obsidian's hotkey storage.
   - If command palette clarity matters, rename the display label for the old above command to indicate it is legacy or
     unbound, but do not rely on a display-name change for behavior.
   - Do not remove the above handler unless it is clearly dead code after verifying no other internal callers depend on
     it; leaving it available is lower risk than deleting a working primitive.

## Files Expected To Change

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

## Files Expected Not To Change

- Markdown notes and unrelated dirty/untracked vault files.
- `obsidian_vimrc.md`
- Other community plugins.
- `bob-cli` source files and memory files.

## Implementation Steps

1. Re-check live state before editing:
   - `git -C /home/bryan/bob status --short --branch`
   - `git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json`

2. Inspect the exact current hotkey JSON entries and plugin fallback functions.
   - Confirm `Alt+O` is still the working shifted chord in `hotkeys.json`.
   - Confirm no other command in `hotkeys.json` already uses `Alt+O`.

3. Edit `hotkeys.json`.
   - Move the `Alt+O` binding to `task-status-cycler:open-child-bullet-line-below`.
   - Remove/unbind `task-status-cycler:open-child-bullet-line-above`.
   - Preserve the file's existing formatting style and avoid unrelated hotkey churn.

4. Edit `main.js`.
   - Change keydown direction resolution so shifted and unshifted `Alt+o` variants resolve to `"below"`.
   - Change beforeinput direction resolution so both `ø` and `Ø` resolve to `"below"`.
   - Leave dispatch, Vim resolver, insertion handlers, and the child-prefix helper unchanged unless validation exposes a
     separate issue.

5. Static validation:
   - `node --check /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json`

6. Focused harness validation:
   - Mock the plugin's event dispatch path and fake CodeMirror object.
   - Assert `Alt+Shift+KeyO`, `key: "Ø"`, and `beforeinput data: "Ø"` all call the below handler.
   - Assert plain `Alt+KeyO`, `key: "ø"`, and `beforeinput data: "ø"` also call below if they ever arrive.
   - Assert insert/visual/replace mode still falls through without preventing default.
   - Assert duplicate window/document listeners still do not double-insert.
   - Assert the direct below command still performs below insertion.

7. Manual smoke test in GUI Obsidian after reloading the plugin or hotkeys.
   - Vim normal mode: `Alt+Shift+o` creates a child bullet below and enters insert mode.
   - Vim normal mode: `Alt+Shift+o` no longer creates a line above.
   - Vim insert mode: `Alt+Shift+o` falls through.
   - Existing `Alt+]` / `Alt+[`, plain `o` / `O`, and task-status-cycler mappings still work.
   - Plain `Alt+o` is not part of the done criteria; if it happens to work, it should also create below.

8. Review and commit.
   - Confirm the final vault diff is limited to `hotkeys.json` and `task-status-cycler/main.js`.
   - Stage only those files.
   - Commit through the required SASE git commit workflow.
   - Leave unrelated dirty note files untouched.

## Risks And Mitigations

- Risk: Obsidian stores shifted letters as `"O"` today, but rewrites hotkeys after a GUI settings save.
  - Mitigation: match the existing live serialization (`"key": "O"`) and manually verify in the GUI.

- Risk: The fallback intercepts the shifted chord before native hotkey dispatch.
  - Mitigation: explicitly change the fallback's shifted mapping to below, so either path produces the desired result.

- Risk: Removing the above binding surprises future command-palette use.
  - Mitigation: keep the above command implementation available but unbound; only the keymap behavior changes.

- Risk: Plain `Alt+o` remains configured and confusing.
  - Mitigation: move the official below binding to `Alt+O`; treat plain `Alt+o` as opportunistic fallback only, not as
    the user-facing contract.

## Done Criteria

- `Alt+Shift+o` creates a child bullet below in Vim normal mode.
- `Alt+Shift+o` no longer creates a child bullet above.
- The child-bullet prefix, cursor placement, and insert-mode transition match the current below handler.
- Non-normal Vim modes still fall through.
- Static checks and the focused harness pass.
- The final vault diff is limited to the plugin and hotkey config.
- The vault change is committed with unrelated dirty files untouched.
