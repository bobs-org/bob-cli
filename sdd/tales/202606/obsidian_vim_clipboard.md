---
create_time: 2026-06-14 09:59:06
status: wip
prompt: sdd/prompts/202606/obsidian_vim_clipboard.md
---
# Obsidian Vim Operator Clipboard Plan

## Goal

Make Obsidian Vim-mode `y` and `d` operator results land in the system clipboard, while preserving the existing
CodeMirror Vim register behavior that already makes `p` work inside Obsidian.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Required Obsidian memory via:
  `sase memory read long/obsidian.md --reason "Need Obsidian workflow context before planning a fix for vim-mode yank/delete clipboard behavior"`.
- Live vault instructions: `/home/bryan/bob/AGENTS.md`.
- Live vault VimRC settings:
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
  - `/home/bryan/bob/obsidian_vimrc.md`
- Installed Vimrc Support plugin implementation:
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`
- Current vault status:
  - `/home/bryan/bob` already has unrelated dirty files, including `.obsidian/plugins/bob-navigation-hotkeys/main.js`
    and multiple notes.
  - The likely target files for this fix, `obsidian_vimrc.md` and Vimrc Support's `data.json` / `main.js`, are clean and
    tracked.

## Diagnosis

The symptom strongly suggests that CodeMirror Vim's internal yank register is working, but the register is not being
bridged to the OS clipboard:

- `p` works after `y` or `d`, so Obsidian's Vim register did receive the text.
- The system clipboard does not receive the same text, so the missing piece is clipboard integration, not the editor
  operation itself.

The installed `obsidian-vimrc-support` plugin already includes the bridge we need:

- It defines a Vim option named `clipboard`.
- It accepts `unnamed` and `unnamedplus`.
- Enabling that option flips `yankToSystemClipboard` to true.
- Once enabled, the plugin watches document events and syncs the CodeMirror Vim yank register to
  `navigator.clipboard.writeText(...)`.

The active VimRC file, `/home/bryan/bob/obsidian_vimrc.md`, currently has keymaps and `exmap` entries but no
`set clipboard=...` line. That is the most likely root cause.

## Product Decision

Prefer the smallest supported config change first:

```vim
set clipboard=unnamedplus
```

Use `unnamedplus` because it communicates the intended system-clipboard behavior. The installed plugin treats `unnamed`
and `unnamedplus` equivalently, so this should enable its existing clipboard bridge without custom JavaScript or changes
to Bob's navigation/task plugins.

## Implementation Scope

Expected file to edit under `~/bob`:

- `/home/bryan/bob/obsidian_vimrc.md`

Expected planning file in this workspace:

- `sase_plan_obsidian_vim_clipboard.md`

No expected edits unless the minimal config fix fails verification:

- `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`
- `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- bob-cli Rust/source files
- memory files

## Implementation Steps

1. Re-check state immediately before edits.
   - Run `git -C /home/bryan/bob status --short --untracked-files=all`.
   - Confirm `obsidian_vimrc.md` is still clean.
   - Do not touch unrelated dirty files.

2. Add the clipboard option to the active syncable VimRC.
   - Put `set clipboard=unnamedplus` near the top of `/home/bryan/bob/obsidian_vimrc.md`, before mappings.
   - Keep JavaScript VimRC commands disabled.
   - Keep every existing mapping unchanged.

3. Reload the VimRC in Obsidian.
   - Because Vimrc Support marks the VimRC as loaded once per CodeMirror Vim object, a normal file edit alone may not
     take effect in the current session.
   - Preferred manual reload path: restart Obsidian or disable/re-enable Vimrc Support.
   - Alternative live path: run `:source obsidian_vimrc.md` from Obsidian Vim command-line if Vimrc Support is already
     loaded.

4. Verify the expected behavior manually in a Markdown note.
   - Characterwise yank: visually select text, press `y`, then paste into a terminal or another app.
   - Linewise yank: use `yy`, then paste outside Obsidian.
   - Delete operator: use a small reversible delete such as `dw` or visual `d`, then paste outside Obsidian.
   - Confirm Obsidian `p` still works after each operation.
   - Confirm normal external clipboard paste into Obsidian still syncs sensibly with Vim `p`.

5. Run static checks after the edit.
   - `git -C /home/bryan/bob diff --check -- obsidian_vimrc.md`
   - `jq '.' /home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`

6. If the config line does not work, escalate narrowly.
   - Inspect DevTools console for the plugin's "Vim is now set to yank to system clipboard." log or any clipboard
     permission errors.
   - Confirm the option can be applied interactively with `:set clipboard=unnamedplus`.
   - If the option applies but register changes are not captured reliably, patch Vimrc Support's clipboard bridge to run
     after Vim commands more directly, rather than modifying unrelated Bob plugins.
   - Keep any plugin-code fallback focused on the existing `captureYankBuffer` path and test it with a small Node/VM
     harness where practical.

7. Commit only task-related vault edits if implementation changes are made under `~/bob`.
   - `/home/bryan/bob/AGENTS.md` requires a commit before terminating after vault file changes.
   - Use the `/sase_git_commit` workflow.
   - Stage only `obsidian_vimrc.md` for the expected minimal fix.
   - Leave all unrelated dirty and untracked vault files untouched.

## Risks And Mitigations

- Risk: the VimRC edit is not active until reload because Vimrc Support only loads once per CodeMirror Vim object.
  Mitigation: explicitly reload Obsidian/plugin or use `:source obsidian_vimrc.md` during testing.

- Risk: browser/Electron clipboard permissions block writes. Mitigation: check DevTools errors and test an interactive
  `:set clipboard=unnamedplus`; only then consider a focused plugin bridge patch.

- Risk: the bridge updates the Vim yank register from external clipboard content in addition to writing yanks out.
  Mitigation: include a paste-back smoke test so we know `p` still behaves acceptably after external clipboard changes.

- Risk: unrelated vault changes are already present. Mitigation: edit and stage only `obsidian_vimrc.md`, and use
  status/diff checks before any commit.
