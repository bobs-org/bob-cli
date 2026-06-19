---
title: Obsidian Delete-Tab Vim Keymaps Plan
create_time: 2026-06-18 20:58:23
status: planned
prompt: sdd/prompts/202606/obsidian_delete_tab_keymaps.md
---

# Obsidian Delete-Tab Vim Keymaps Plan

## Goal

Add three Vim normal-mode Obsidian keymaps in Bryan's Bob vault:

- `d<`: close every tab to the left of the current tab.
- `d>`: close every tab to the right of the current tab.
- `dD`: close every tab except the current tab.

These should be Vim-mode normal-mode mappings only. They should close Obsidian tabs/leaves, not delete notes or files.
The active tab should remain open and focused.

## Context Reviewed

- Required Obsidian memory was read through the audited memory path:
  `sase memory read obsidian.md --reason "Need Obsidian workflow context before adding vim-mode keymaps"`.
- Live vault instructions: `/home/bryan/bob/AGENTS.md`.
  - The vault is actively synced and may already be dirty.
  - Inspect vault git status before editing.
  - Do not overwrite, revert, stage, or commit unrelated changes.
  - Commit only task-related vault edits with the SASE git workflow before terminating after vault file changes.
- Current live vault state is dirty with unrelated modified notes/config and untracked files. The likely task target
  files are not currently listed as modified:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/obsidian_vimrc.md`
- VimRC Support is installed and configured through
  `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`.
  - Active vimrc file: `/home/bryan/bob/obsidian_vimrc.md`.
  - JavaScript vimrc commands are disabled and should stay disabled.
- Existing tab-management commands already live in `bob-navigation-hotkeys`:
  - `move-tab-left`
  - `move-tab-right`
  - `duplicate-current-tab`
- Existing tab helpers in `bob-navigation-hotkeys/main.js` provide useful precedents:
  - `moveActiveTab(offset)` uses guarded `workspace.activeLeaf`, `leaf.parent || leaf.parentSplit`, `parent.children`,
    `parent.selectTab(leaf)`, and workspace focus fallbacks.
  - `detachWorkspaceLeaf(leaf)` already wraps `leaf.detach()` safely, including async returns.
  - `focusWorkspaceLeaf(leaf)` already handles `revealLeaf`, `setActiveLeaf`, and leaf-level focus fallbacks.
- The Neovim analogue in `~/.config/nvim/lua/config/keymaps/delete_buffers.lua` closes buffers left, right, and "other".
  In Obsidian, the closest product match is the current tab group, not all leaves across every split/group.
- This task does not add or change any `bob` CLI subcommand or option, so `memory/long/cli_rules.md` does not apply.

## Product Decisions

1. Scope tab deletion to the current tab group.
   - "Left" and "right" are visual/order concepts within the current tab container (`parent.children`).
   - Other workspace panes/splits should be left untouched because they are not left/right siblings in the current tab
     strip.

2. Close tabs by detaching sibling leaves, not by deleting files.
   - Reuse the existing `detachWorkspaceLeaf(leaf)` helper.
   - Preserve the active leaf and refocus it after closing siblings.

3. Add Obsidian commands in `bob-navigation-hotkeys` and dispatch them from vimrc.
   - Command IDs should be available in the command palette and reusable by VimRC Support:
     - `close-tabs-left`
     - `close-tabs-right`
     - `close-other-tabs`
   - The mappings themselves belong in `obsidian_vimrc.md`, not `.obsidian/hotkeys.json`, because the requested behavior
     is explicitly Vim normal-mode only.

4. Do not add global hotkeys.
   - Global hotkeys would fire in insert mode, reading mode, and non-editor tabs.
   - That would violate the requested Vim normal-mode behavior.

5. Keep behavior quiet for no-op states.
   - If there is no active leaf, no tab-group parent, only one tab, or no matching tabs on the requested side, return
     false/no-op without notices.
   - This matches the existing tab-move command style and avoids notification noise for boundary cases.

6. Use VimRC notation carefully for `d<`.
   - The user-facing chord is `d<`.
   - In the vimrc file, prefer the notation that the installed VimRC Support / CodeMirror Vim parser accepts for a
     literal less-than key. If `nmap d< ...` is accepted, use that. If the parser requires escaping, use the equivalent
     literal-key notation such as `d<lt>` while preserving the actual typed chord `d<`.
   - `d>` and `dD` can be mapped directly.

## Technical Design

Add command registrations near the existing tab commands in
`/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`:

```js
this.addCommand({
  id: "close-tabs-left",
  name: "Close tabs to the left",
  callback: () => this.closeSiblingTabs("left"),
});

this.addCommand({
  id: "close-tabs-right",
  name: "Close tabs to the right",
  callback: () => this.closeSiblingTabs("right"),
});

this.addCommand({
  id: "close-other-tabs",
  name: "Close other tabs",
  callback: () => this.closeSiblingTabs("others"),
});
```

Add a shared helper near the existing tab helper methods:

```js
async closeSiblingTabs(scope) {
  const workspace = this.app && this.app.workspace;
  const activeLeaf = workspace && workspace.activeLeaf;
  const parent = activeLeaf && (activeLeaf.parent || activeLeaf.parentSplit);
  const children = parent && parent.children;
  ...
}
```

Expected helper behavior:

1. Guard against missing workspace, active leaf, parent, or `parent.children`.
2. Copy `parent.children` before detaching anything so iteration is stable while Obsidian mutates the tab group.
3. Locate the active leaf index in that snapshot.
4. Build the close list:
   - `left`: leaves before the active index.
   - `right`: leaves after the active index.
   - `others`: all leaves except the active leaf.
5. Detach leaves sequentially with `await this.detachWorkspaceLeaf(leaf)`.
   - Sequential detach is simpler and safer than parallel detach because each detach mutates workspace layout.
6. Refocus the active leaf with `await this.focusWorkspaceLeaf(activeLeaf)` after successful or attempted sibling
   detachments.
7. Return true if at least one leaf was selected for closure, false for no-op states.

Update `/home/bryan/bob/obsidian_vimrc.md`:

```vim
exmap bob_close_tabs_left obcommand bob-navigation-hotkeys:close-tabs-left
exmap bob_close_tabs_right obcommand bob-navigation-hotkeys:close-tabs-right
exmap bob_close_other_tabs obcommand bob-navigation-hotkeys:close-other-tabs

nmap d< :bob_close_tabs_left<CR>
nmap d> :bob_close_tabs_right<CR>
nmap dD :bob_close_other_tabs<CR>
```

If `d<` needs escaped literal notation in this parser, the first mapping should use the parser-compatible equivalent
while still producing the typed chord `d<`.

## Implementation Scope

Expected files to edit under `/home/bryan/bob`:

- `.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `obsidian_vimrc.md`

No expected edits:

- `.obsidian/hotkeys.json`
- `.obsidian/plugins/obsidian-vimrc-support/data.json`
- `.obsidian/community-plugins.json`
- plugin manifests
- Markdown notes, templates, or Bases files
- memory files
- bob-cli Rust/script/docs/tests

The plan artifact itself lives in the bob-cli workspace as `sase_plan_obsidian_delete_tab_keymaps.md`.

## Implementation Steps

1. Re-check state immediately before editing.
   - `git status --short` in the bob-cli workspace.
   - `git -C /home/bryan/bob status --short --untracked-files=all`.
   - Targeted diffs for `.obsidian/plugins/bob-navigation-hotkeys/main.js` and `obsidian_vimrc.md`.

2. Add the three command registrations to `bob-navigation-hotkeys/main.js`.
   - Place them near existing `move-tab-left`, `move-tab-right`, and `duplicate-current-tab` registrations.
   - Use concise command names that describe closing tabs, not deleting files.

3. Add `closeSiblingTabs(scope)` near the existing tab helper methods.
   - Reuse `detachWorkspaceLeaf` and `focusWorkspaceLeaf`.
   - Snapshot `parent.children` before closing leaves.
   - Keep all workspace API calls feature-detected and guarded, matching existing tab helper style.

4. Update `obsidian_vimrc.md`.
   - Add the three `exmap` wrappers near the existing navigation/tab-related command mappings.
   - Add the three `nmap` lines in the normal-mode mapping block.
   - Verify the literal `<` mapping notation for `d<`.

5. Validate statically.
   - `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js obsidian_vimrc.md`

6. Review the targeted diff.
   - Confirm `.obsidian/hotkeys.json` is untouched.
   - Confirm only the new commands/helper and vimrc mappings changed.
   - Confirm unrelated dirty vault files remain unstaged and unmodified by this task.

7. Manual Obsidian smoke test after reloading the plugin/app or re-sourcing the vimrc.
   - Open at least four tabs in one tab group.
   - With an interior tab active, `d<` closes only tabs to its left and keeps the current tab active.
   - Recreate tabs; `d>` closes only tabs to its right and keeps the current tab active.
   - Recreate tabs; `dD` closes every sibling tab and leaves only the current tab in that group.
   - Boundary checks: leftmost `d<`, rightmost `d>`, and single-tab `dD` no-op without errors or notice spam.
   - Confirm tabs in other tab groups/splits are unaffected.
   - Confirm insert mode still types normally and does not trigger these mappings.
   - Confirm ordinary Vim delete operations such as `dw`, `dd`, and `D` still work.

8. Commit task-related vault edits before terminating after implementation.
   - Use the required SASE git commit workflow.
   - Stage only:
     - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
     - `obsidian_vimrc.md`
   - Review the staged diff before committing.
   - Leave unrelated dirty and untracked vault files untouched.

## Risks And Mitigations

- Risk: `d`-prefixed mappings interfere with native Vim delete commands.
  - Mitigation: add only exact multi-key normal-mode mappings; smoke-test `dw`, `dd`, and `D` after reload.

- Risk: Literal `<` is parsed as special-key syntax in the vimrc file.
  - Mitigation: validate the accepted VimRC Support notation and use the parser-compatible representation for the same
    typed chord.

- Risk: Closing leaves mutates `parent.children` while iterating.
  - Mitigation: snapshot the sibling array before detaching leaves and close sequentially.

- Risk: Obsidian workspace internals are not a stable public API.
  - Mitigation: mirror the existing guarded `moveActiveTab` implementation style and feature-detect parent/children,
    `selectTab`, focus, and detach APIs.

- Risk: Accidentally closing tabs in other splits/groups would be surprising.
  - Mitigation: scope strictly to the active leaf's parent tab group.

- Risk: Dirty vault state could be accidentally staged or overwritten.
  - Mitigation: re-check status before editing and stage only the two task files through the SASE git workflow.

## Done Criteria

- Three new `bob-navigation-hotkeys` commands exist for close-tabs-left, close-tabs-right, and close-other-tabs.
- `obsidian_vimrc.md` maps normal-mode `d<`, `d>`, and `dD` to those commands.
- Static validation passes.
- Manual smoke testing confirms correct tab-group behavior and no regression to core Vim delete commands.
- The final vault commit contains only the intended plugin and vimrc changes.
