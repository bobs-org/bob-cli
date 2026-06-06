---
create_time: 2026-06-06 14:36:04
status: wip
prompt: sdd/prompts/202606/obsidian_yank_path_keymap_fix.md
---
# Obsidian Yank Path Keymap Fix Plan

## Goal

Diagnose why pressing `,yr` in Obsidian after restart did not copy the active note's vault-relative path, then fix the
runtime path so the six yank-path keypresses work reliably in Obsidian normal mode.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Required Obsidian long memory via:
  `sase memory read long/obsidian.md --reason "Diagnose Obsidian vimrc and plugin command behavior for path yank keymaps"`.
- Approved implementation plan: `sdd/tales/202606/obsidian_yank_path_keymaps.md`.
- Live vault agent rules: `/home/bryan/bob/AGENTS.md`.
- Live vault status, plugin enablement, Vim mode config, VimRC Support settings, and current `obsidian_vimrc.md`.
- Current path-yank implementation in `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- Current VimRC Support implementation in `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`.

## Current Findings

- The live vault commit is `bb13eeb feat: add Obsidian path yank keymaps`, and it contains only:
  - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `obsidian_vimrc.md`
- The live vault still has unrelated pre-existing dirty files. They must remain untouched.
- `obsidian-vimrc-support` and `bob-navigation-hotkeys` are enabled in `.obsidian/community-plugins.json`.
- Obsidian Vim mode is enabled in `.obsidian/app.json`.
- VimRC Support is configured to load `obsidian_vimrc.md`, and that file contains:
  - `exmap bob_yank_relative_path obcommand bob-navigation-hotkeys:yank-relative-path`
  - `nmap ,yr :bob_yank_relative_path<CR>`
- The Bob Navigation Hotkeys plugin registers the expected command ID: `bob-navigation-hotkeys:yank-relative-path`.
- A Node harness with Obsidian APIs stubbed verified that calling `yankActiveFilePath("relative")` writes
  `2026/20260606_day.md` to the clipboard and produces the success notice.
- Therefore the path formatter and command callback are not the primary failure. The failure is most likely in the Vim
  dispatch path from `,yr` to the Obsidian command.

## Working Diagnosis

The first implementation depended on VimRC Support to translate comma-prefixed normal-mode mappings into Ex commands,
then into `obcommand` calls. That stack is more fragile than the local plugin command itself. The symptom "nothing
happened" after restart is consistent with the mapping not firing at all, not with the command failing after it fires:
if the command callback fired, the implementation would either copy the relative path and show `Copied relative path`,
or show a failure notice.

The fix should make `bob-navigation-hotkeys` own the runtime Vim mappings directly, using the same
`window.CodeMirrorAdapter.Vim` API pattern already used successfully by `bob-ledger-tools`. The VimRC file can remain as
a readable/syncable backup, but the working key dispatch should not rely solely on VimRC Support's `nmap`/`exmap` chain.

## Implementation Plan

1. Re-check live vault state immediately before edits.
   - Run `git -C /home/bryan/bob status --short --untracked-files=all`.
   - Confirm target files have no unrelated pre-existing diffs.

2. Add direct Vim mapping registration to `bob-navigation-hotkeys`.
   - Add a small `YANK_PATH_VIM_MAPPINGS` table mapping:
     - `,ya` -> `absolute-tilde`
     - `,yA` -> `absolute`
     - `,yb` -> `basename`
     - `,yB` -> `basename-no-extension`
     - `,yd` -> `parent-directory`
     - `,yr` -> `relative`
   - Register CodeMirror Vim actions through `window.CodeMirrorAdapter.Vim.defineAction`.
   - Map keys through `vim.mapCommand(mapping.keys, "action", actionName, {}, { context: "normal" })`.
   - Register on layout ready, and retry on active leaf changes until the Vim adapter exists.
   - Keep registration idempotent so plugin reloads do not duplicate work.

3. Make command execution observable and robust.
   - Keep Obsidian command palette commands as the canonical command surface.
   - Ensure the direct Vim action calls `this.yankActiveFilePath(kind)` and catches/report failures with a Notice.
   - Prefer the existing clipboard path initially; only change clipboard implementation if testing shows direct command
     execution reaches clipboard but clipboard writes fail.

4. Keep `obsidian_vimrc.md` conservative.
   - Leave the existing `exmap`/`nmap` entries unless direct registration makes a duplicate behavior problem likely.
   - If duplicate execution is observed or clearly likely, remove only the six `,y...` VimRC `nmap` lines while leaving
     the `exmap` wrappers available for manual Ex command use.

5. Validate without the GUI where possible.
   - `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js obsidian_vimrc.md`
   - Node harness for:
     - path command callback writes the expected relative path;
     - direct Vim registration calls `defineAction` and `mapCommand` for `,yr` and the other five mappings;
     - registration is idempotent.

6. Commit only task-related vault edits if files under `~/bob` are changed.
   - Required by `/home/bryan/bob/AGENTS.md`.
   - Use the `/sase_git_commit` workflow.
   - Stage only:
     - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
     - `obsidian_vimrc.md` if it changes
   - Leave unrelated dirty/untracked vault files untouched.

## Manual Smoke Test For Bryan

After the fix is installed and Obsidian is reloaded:

- Open a Markdown note in editing/source mode with Vim normal mode active.
- Press `,yr`; expected clipboard content is the vault-relative active note path, for example `2026/20260606_day.md`,
  and Obsidian should show `Copied relative path`.
- Also test the command palette command `Bob Navigation Hotkeys: Yank relative path`.
- If the command palette works but the key still does not, the remaining issue is CodeMirror Vim key interception.
- If neither works, the remaining issue is command registration or clipboard access.

## Risks

- The direct mappings and VimRC mappings could both fire if VimRC Support starts honoring the comma mappings. The
  practical impact would be two identical clipboard writes and two notices; remove the six VimRC `nmap` lines if needed.
- CodeMirror Vim may not be available at plugin load time. Retrying after layout and active-leaf changes mitigates this.
- Snap/Electron clipboard behavior can differ from the Node harness. The existing Notice paths should make that visible
  if it occurs.
