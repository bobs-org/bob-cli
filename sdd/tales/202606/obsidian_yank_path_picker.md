---
create_time: 2026-06-06 14:58:37
status: done
prompt: sdd/prompts/202606/obsidian_yank_path_picker.md
---
# Obsidian Yank Path Picker Plan

## Goal

Replace the failed comma-prefixed Vim path-yank mappings with one Obsidian command, bound to `Mod+Y` (`Cmd+Y` on macOS),
that opens a picker. The picker should use the same modal pattern as the existing child-note picker and let the user
choose which active-file path representation to copy.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Required Obsidian long memory via:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and plugin workflow context before planning yank-path keymap popup change"`.
- Live vault rules: `/home/bryan/bob/AGENTS.md`.
- Prior approved/implemented plan: `sdd/tales/202606/obsidian_yank_path_keymap_fix.md`.
- Live vault status and diffs for:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`
  - `/home/bryan/bob/.obsidian/hotkeys.json`
  - `/home/bryan/bob/obsidian_vimrc.md`
- Existing picker implementation:
  - `FilteredPickerModal`
  - `ChildNotePickerModal`
  - `LinkCandidatePickerModal`
- Existing path-copy implementation:
  - `YANK_PATH_COMMANDS`
  - `getYankPathText`
  - `yankActiveFilePath`
  - `getActiveFileYankPath`
  - clipboard/Notice handling

## Current Findings

- The path-copy formatter and clipboard command path are already centralized and reusable.
- The previous fix added direct CodeMirror Vim mappings in `bob-navigation-hotkeys`, but the new user direction is to
  stop relying on that path and use one normal Obsidian hotkey plus a picker.
- `obsidian_vimrc.md` still contains the six stale `,y...` mappings and matching `exmap` wrappers from the earlier
  implementation.
- `hotkeys.json` is already dirty before this task. Its current dirty hunk appears unrelated to yank-path behavior.
- `Mod+Y` is not currently present in the live `hotkeys.json`.
- The existing `bob-cnp-*` modal CSS is generic enough for another filtered picker without requiring a new visual
  system.

## Design

### Command Surface

Add one new public command to `bob-navigation-hotkeys`:

- id: `copy-active-file-path`
- name: `Copy active file path`
- default hotkey: `{ modifiers: ["Mod"], key: "Y" }`
- callback: open the new path picker

Use a plugin-level default hotkey instead of editing `.obsidian/hotkeys.json` initially. That avoids mixing this task
with the file's pre-existing user/sync changes and still gives Obsidian a real `Cmd+Y` binding for the command. If
manual Obsidian testing later shows default plugin hotkeys are ignored in this vault, then update `hotkeys.json` in a
separate, carefully isolated step.

Keep the six existing command-palette commands for direct path copies unless testing shows they interfere. They are not
keymaps, and keeping them preserves the current public command surface while the new picker becomes the only new
keyboard workflow.

### Picker Behavior

Add a `YankPathPickerModal` that extends `FilteredPickerModal`, like `ChildNotePickerModal` and
`LinkCandidatePickerModal`.

The picker should list the six existing path kinds:

- Absolute path with tilde (`absolute-tilde`)
- Absolute path (`absolute`)
- Basename (`basename`)
- Basename without extension (`basename-no-extension`)
- Parent directory (`parent-directory`)
- Relative path (`relative`)

For each row:

- show a clear title, such as `Relative path`;
- show a preview of the exact text that would be copied when available;
- show an unavailable/error label for runtime-only failures, such as missing vault base path for absolute paths;
- filter by title, kind, and preview text.

On selection:

- call the existing `yankActiveFilePath(kind)`;
- close the modal only when the copy succeeds;
- keep current Notice behavior (`Copied relative path`, `Clipboard is unavailable`, etc.).

If no Markdown file is active, opening the command should show `No active markdown file` and not open the picker.

### Remove Failed Vim Dispatch

Remove the direct CodeMirror Vim registration added by the previous fix:

- `YANK_PATH_VIM_MAPPINGS`
- `yankPathVimMappingsRegistered`
- `registerYankPathVimMappingsWhenReady`
- `registerYankPathVimMappings`
- `handleYankPathVimAction`
- the `onload` call to register the Vim mappings
- the helper export for `YANK_PATH_VIM_MAPPINGS`

Update `obsidian_vimrc.md` to remove the stale yank-path key path:

- remove the six `nmap ,y...` mappings;
- remove the six matching `exmap bob_yank_...` wrappers, since the new supported path is the Obsidian command/picker.

Leave unrelated VimRC mappings intact.

### Files Expected To Change

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/obsidian_vimrc.md`

Avoid editing `/home/bryan/bob/.obsidian/hotkeys.json` unless default command hotkey registration proves insufficient.
Avoid editing styles unless the existing picker classes cannot present the path preview cleanly.

## Validation Plan

Run static validation:

- `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js obsidian_vimrc.md`

Run a focused Node harness with Obsidian APIs stubbed:

- plugin load registers `copy-active-file-path` with default `Mod+Y`;
- opening the picker command with no active Markdown file shows the expected Notice and does not create a modal;
- opening the picker with an active Markdown file creates six choices;
- each choice previews the expected path text for a sample file and vault base path;
- selecting `relative` writes the sample relative path to the clipboard;
- selecting absolute variants handles vault-base-path availability;
- the removed Vim mapping registration no longer calls `window.CodeMirrorAdapter.Vim.defineAction` or `mapCommand`.

Validate config syntax:

- if `obsidian_vimrc.md` changes, inspect the resulting file for only the intended mapping removals;
- if `hotkeys.json` unexpectedly changes, run `jq '.' /home/bryan/bob/.obsidian/hotkeys.json` and inspect/stage only
  task-related hunks.

Manual smoke test for Bryan after reload:

- Open a Markdown note in Obsidian.
- Press `Cmd+Y`.
- Expected: a picker opens with the six path-copy options.
- Choose `Relative path`.
- Expected: clipboard contains the active note's vault-relative path and Obsidian shows `Copied relative path`.
- Verify stale `,yr` behavior is no longer part of the supported workflow.

## Commit Plan

Because this changes files under `~/bob`, follow `/home/bryan/bob/AGENTS.md`:

- inspect `git -C /home/bryan/bob status --short --untracked-files=all` before edits;
- preserve unrelated dirty files;
- stage only task-related files;
- commit with the `sase_git_commit` workflow after implementation and validation.

Expected commit type: `feat`, because this replaces a broken keymap strategy with a new user-facing picker command.

## Risks

- Obsidian may treat `Mod+Y` as conflicting with a built-in redo key on some platforms. The current live `hotkeys.json`
  does not show `Mod+Y`, and the user explicitly requested `Cmd+Y`, so proceed but verify manually.
- Plugin-level default hotkeys may not override a user-customized vault configuration. If this happens, add an explicit
  `hotkeys.json` entry only after isolating the pre-existing dirty hunk.
- Removing VimRC yank mappings intentionally retires the previous workflow. This is aligned with the new direction but
  should be mentioned in the final result.
- The picker preview for absolute paths depends on `vault.adapter.getBasePath()`, which may be unavailable in some
  Obsidian runtimes; those choices should render as unavailable rather than breaking the modal.
