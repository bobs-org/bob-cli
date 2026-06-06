---
title: Obsidian Yank Path Keymaps Plan
create_time: 2026-06-06 14:12:51
status: planned
prompt: sdd/prompts/202606/obsidian_yank_path_keymaps.md
---

# Obsidian Yank Path Keymaps Plan

## Goal

Implement the executable keymaps from Bryan's Neovim `yank_path.lua` in the `~/bob` Obsidian vault.

The Neovim source defines a `<leader>y` group and six useful mappings:

- `<leader>ya`: copy absolute path, replacing the home directory with `~`.
- `<leader>yA`: copy full absolute path.
- `<leader>yb`: copy basename including extension.
- `<leader>yB`: copy basename without extension.
- `<leader>yd`: copy parent directory relative to the current working directory.
- `<leader>yr`: copy file path relative to the current working directory.

Bryan's Neovim leader is comma, so the Obsidian normal-mode keypresses should be `,ya`, `,yA`, `,yb`, `,yB`, `,yd`, and
`,yr`.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Required Obsidian long memory via:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and configuration workflow before implementing matching keymaps"`.
- Neovim source: `/home/bryan/.local/share/chezmoi/home/dot_config/nvim/lua/config/keymaps/yank_path.lua`.
- Vault instructions: `/home/bryan/bob/AGENTS.md`.
- Live Obsidian VimRC configuration: `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`.
- Live syncable VimRC file: `/home/bryan/bob/obsidian_vimrc.md`.
- Existing custom command plugin: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

Important current-state findings:

- The vault is dirty before this task. Existing unrelated changes include `.obsidian/community-plugins.json`,
  `.obsidian/hotkeys.json`, `.obsidian/plugins/block-id-prompt/main.js`, `.obsidian/plugins/task-status-cycler/main.js`,
  several notes, and untracked notes. These must be preserved and left out of any task commit.
- The VimRC Support plugin now reads `obsidian_vimrc.md`, not `.obsidian.vimrc`, so new normal-mode mappings should be
  added to `obsidian_vimrc.md`.
- JavaScript VimRC commands are disabled and should stay disabled.
- `bob-navigation-hotkeys` already registers Obsidian commands and is the right local integration point for small
  Bob-specific command behavior.
- The Neovim `<leader>y` mapping itself is a descriptive no-op group. Obsidian does not need a bare `,y` mapping unless
  the runtime requires one for prefix handling, and adding one could block future `,y...` combinations.

## Product And Behavior Decisions

1. Expose real Obsidian commands for every path-yank operation.
   - Add command IDs to `bob-navigation-hotkeys` such as:
     - `yank-absolute-path-tilde`
     - `yank-absolute-path`
     - `yank-basename`
     - `yank-basename-without-extension`
     - `yank-parent-directory`
     - `yank-relative-path`
   - This makes the actions available both through VimRC mappings and the Obsidian command palette.

2. Keep all key dispatch in the syncable VimRC file.
   - Add `exmap` wrappers in `obsidian_vimrc.md` that call the new Obsidian commands with `obcommand`.
   - Add normal-mode mappings for `,ya`, `,yA`, `,yb`, `,yB`, `,yd`, and `,yr`.
   - Prefer explicit comma mappings over relying on a newly introduced `let mapleader = ","` setting, because the live
     VimRC currently has no leader setting and explicit mappings exactly preserve Bryan's current Neovim keypresses.

3. Treat the vault root as Obsidian's current working directory equivalent.
   - Neovim computes relative paths from `vim.fn.getcwd()`.
   - Obsidian has no per-window CWD. For `~/bob`, the stable analogue is the vault root.
   - Therefore:
     - relative path is the active Markdown file's vault-relative path, for example `2026/20260606_day.md`;
     - parent directory is the active file's vault-relative parent folder, for example `2026`;
     - a root-level note's parent directory should copy an empty string, which matches Neovim's CWD-relative behavior
       for a file in the CWD.

4. Compute absolute paths only when the vault adapter exposes a filesystem base path.
   - On desktop, the vault adapter should expose `getBasePath()`, yielding `/home/bryan/bob`.
   - Absolute path variants should join that base path with the vault-relative file path and normalize separators.
   - If the base path is unavailable, show a Notice instead of copying a fake absolute path. The plugin manifest is
     currently not desktop-only, so the command should fail gracefully on unsupported runtimes.

5. Use the system clipboard directly from the plugin.
   - Prefer `navigator.clipboard.writeText(value)` where available.
   - Show a short success Notice naming the copied variant, and a failure Notice if clipboard access is unavailable or
     rejected.
   - Keep the command behavior read-only with respect to notes.

6. Preserve existing command and hotkey behavior.
   - Do not edit `.obsidian/hotkeys.json`; these Vim-style mappings belong in `obsidian_vimrc.md`.
   - Do not change current navigation mappings for `-`, `[[`, `]]`, or `!`.
   - Do not enable VimRC JavaScript support.

## Implementation Scope

Expected files to edit under `~/bob`:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/obsidian_vimrc.md`

Expected generated or planning file in the bob-cli workspace:

- `sase_plan_obsidian_yank_path_keymaps.md`

No expected edits:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
- `/home/bryan/bob/.obsidian/community-plugins.json`
- `/home/bryan/bob/.obsidian.vimrc`
- Obsidian notes or templates
- Memory files
- bob-cli Rust/Python source

## Implementation Steps

1. Re-check file state immediately before editing.
   - Run `git -C /home/bryan/bob status --short --untracked-files=all`.
   - Inspect targeted diffs for `bob-navigation-hotkeys/main.js` and `obsidian_vimrc.md`.
   - Confirm those files have no unrelated pre-existing changes.

2. Add path-formatting helpers to `bob-navigation-hotkeys/main.js`.
   - Reuse the existing active Markdown file helper.
   - Add small helpers for:
     - vault-relative active path;
     - basename with extension;
     - basename without extension;
     - vault-relative parent directory;
     - absolute path from `adapter.getBasePath()`;
     - tilde-compacted absolute path for paths under the user's home directory.
   - Keep these helpers pure where practical and export them through the existing `module.exports.helpers` object if
     useful for tests.

3. Add clipboard command plumbing.
   - Add an async method such as `yankActiveFilePath(kind)`.
   - Use `await navigator.clipboard.writeText(text)`.
   - Return false and show a Notice when there is no active Markdown file, no filesystem base path for absolute
     variants, or no clipboard API.
   - Register the six new commands in `onload()`.

4. Update `obsidian_vimrc.md`.
   - Add `exmap` lines for the six command IDs.
   - Add normal-mode mappings:
     - `nmap ,ya :bob_yank_abs_tilde<CR>`
     - `nmap ,yA :bob_yank_abs<CR>`
     - `nmap ,yb :bob_yank_basename<CR>`
     - `nmap ,yB :bob_yank_basename_no_ext<CR>`
     - `nmap ,yd :bob_yank_parent_dir<CR>`
     - `nmap ,yr :bob_yank_relative_path<CR>`
   - Keep the existing command mappings intact.

5. Validate statically.
   - `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js obsidian_vimrc.md`

6. Add focused Node-level tests if the helper changes are nontrivial.
   - Use the existing exported helper pattern to test formatting behavior without trying to boot Obsidian.
   - Cover root-level files, nested files, extension stripping, absolute path construction, and home-directory tilde
     compaction.
   - If adding tests is disproportionate because the plugin has no test harness, keep validation to syntax and helper
     spot checks from `node`.

7. Manual Obsidian smoke test after reload.
   - Reload Obsidian or disable and re-enable `bob-navigation-hotkeys` and VimRC Support.
   - In Vim normal mode on a nested Markdown note:
     - `,ya` copies a `~`-prefixed absolute path under `/home/bryan`.
     - `,yA` copies the full absolute path.
     - `,yb` copies `filename.md`.
     - `,yB` copies `filename`.
     - `,yd` copies the vault-relative parent folder.
     - `,yr` copies the vault-relative file path.
   - Repeat `,yd` on a root-level note and confirm it copies an empty string.

8. Commit only task-related vault edits before terminating.
   - Required by `/home/bryan/bob/AGENTS.md` after modifying files under `~/bob`.
   - Use the `/sase_git_commit` workflow, staging only:
     - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
     - `obsidian_vimrc.md`
   - Leave all unrelated dirty and untracked vault files untouched.

## Risks And Mitigations

- Risk: CodeMirror Vim treats comma-prefixed multi-key mappings differently from Neovim leader mappings. Mitigation: use
  explicit `,y...` mappings and manually smoke test after reload.

- Risk: Absolute filesystem paths are unavailable outside desktop FileSystemAdapter-backed vaults. Mitigation: show a
  Notice and do not copy a misleading path.

- Risk: Clipboard writes can fail if Obsidian runtime permissions differ. Mitigation: surface a Notice on failure; keep
  commands async.

- Risk: The existing vault has unrelated dirty changes. Mitigation: inspect status before editing, avoid unrelated
  files, and stage only task files.

- Risk: Root-level parent directory behavior may look surprising because it copies an empty string. Mitigation: this
  matches the Neovim CWD-relative semantics for a file in the current directory.
