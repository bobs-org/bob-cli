---
create_time: 2026-06-13 06:58:54
status: done
prompt: sdd/prompts/202606/obsidian_alt_t_duplicate_tab.md
---
# Obsidian Alt+T Duplicate-Tab Keymap

## Goal

Add a new global Obsidian keymap, `Alt+T`, that duplicates the currently active Obsidian tab in Bryan's Bob vault.

The duplicate should be a new tab in the same workspace context, focused after creation, and should preserve the current
tab's view state as closely as Obsidian allows. This should work for Markdown notes and should not be artificially
limited to Markdown when Obsidian can duplicate the active view state for other tab types.

## Context Reviewed

- Long-term Obsidian memory was read through the required audited path:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault plugin and keymap conventions before planning Alt+T duplicate-tab support"`.
- The live vault is `/home/bryan/bob`.
- `/home/bryan/bob/AGENTS.md` requires checking vault git status before editing, preserving unrelated dirty changes, and
  committing vault edits via the SASE git workflow before terminating.
- Current vault status has unrelated changes only:
  - modified `bob_projects_clean_bad_links.md`
  - untracked `2026/20260613.md`
- Existing keymap surfaces:
  - `/home/bryan/bob/.obsidian/hotkeys.json` for global hotkeys.
  - `/home/bryan/bob/obsidian_vimrc.md` for Vim normal-mode mappings.
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` for bespoke navigation and workspace commands.
- `Alt+T` is not currently bound in `hotkeys.json`.
- `bob-navigation-hotkeys` already owns tab-level commands:
  - `move-tab-left` bound to `Ctrl+Shift+,`
  - `move-tab-right` bound to `Ctrl+Shift+.`
  - `moveActiveTab(offset)` already uses guarded Obsidian workspace leaf/tab internals.
- This task does not add or change any `bob` CLI subcommand or option, so the project `memory/long/cli_rules.md` trigger
  does not apply.

## Product Decisions

1. Implement this as a `bob-navigation-hotkeys` command, not a Vim-only mapping.
   - Duplicating a tab is a workspace operation, not an editor operation.
   - A global hotkey works in Vim normal mode, insert mode, reading view, and non-editor views.
   - The navigation plugin already owns adjacent tab-management behavior.

2. Bind `Alt+T` in `hotkeys.json`, not in `obsidian_vimrc.md`.
   - This matches the existing non-editor-safe tab movement bindings.
   - Existing Alt-family bindings use explicit `"Alt"` modifiers in `hotkeys.json`, so the new binding should follow
     that convention.

3. Duplicate the active leaf's view state, not just the active Markdown file.
   - The user asked for duplicating the current Obsidian tab, which is broader than "open this Markdown file again".
   - `WorkspaceLeaf.getViewState()` plus `WorkspaceLeaf.setViewState(...)` is the natural Obsidian-level representation
     of a tab.
   - If view-state duplication is unavailable or fails, fall back only when there is a safe active Markdown file path;
     otherwise show a concise notice rather than opening an unrelated blank tab.

4. Prefer a new tab in the same tab group and focus it.
   - Use `workspace.getLeaf("tab")` when available, with a guarded fallback for older/alternate Obsidian behavior.
   - If Obsidian creates the duplicate tab in the same parent but not adjacent to the source tab, place it immediately
     after the source using the same guarded `parent.children` / `parent.selectTab` style already used by
     `moveActiveTab`.
   - If parent internals are unavailable, accept Obsidian's created-tab placement instead of failing the command.

## Technical Design

Add one new command to `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`:

```js
this.addCommand({
  id: "duplicate-current-tab",
  name: "Duplicate current tab",
  callback: () => this.duplicateCurrentTab(),
});
```

Add a method near the existing tab helpers:

```js
async duplicateCurrentTab() {
  ...
}
```

Expected behavior:

1. Get `workspace` and `sourceLeaf = workspace.activeLeaf`; fail gracefully if unavailable.
2. Read the current tab's view state using `sourceLeaf.getViewState()` when available.
3. Create a target leaf:
   - prefer `workspace.getLeaf("tab")`
   - fallback to `workspace.getLeaf(true)` only if the `"tab"` path is unavailable or throws
4. Duplicate the state:
   - clone the view state before mutating it
   - set `active: true` on the cloned state where appropriate
   - call `await targetLeaf.setViewState(clonedState)`
5. Focus the duplicate:
   - call `workspace.setActiveLeaf(targetLeaf, { focus: true })` when available, with the plugin's existing one-argument
     fallback style
   - call `targetLeaf.focus()` if that API exists and focus still needs help
6. Place the duplicate next to the source when the source and target share a parent tab container:
   - remove `targetLeaf` from `parent.children`
   - insert it immediately after `sourceLeaf`
   - call `parent.selectTab(targetLeaf)` when available
   - otherwise use the same DOM/layout/focus fallback style as `moveActiveTab`
7. On failure:
   - if the active view is a Markdown file and `targetLeaf.openFile` is available, open that file in the target tab as a
     degraded fallback
   - otherwise show `new Notice("Could not duplicate current tab")`
   - avoid notice spam for simple no-op states like no active leaf

Add one `hotkeys.json` entry, preserving formatting and local ordering around adjacent tab commands:

```json
"bob-navigation-hotkeys:duplicate-current-tab": [
  {
    "modifiers": [
      "Alt"
    ],
    "key": "T"
  }
]
```

No manifest change is expected because this is an additional command in an already enabled local plugin.

## Implementation Scope

Expected files to edit under `/home/bryan/bob`:

- `.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `.obsidian/hotkeys.json`

Expected files not to edit:

- `obsidian_vimrc.md`
- plugin manifests
- `community-plugins.json`
- Markdown note files
- Bob CLI Rust/script/docs/tests
- memory files

The plan artifact itself lives in the bob-cli workspace as `sase_plan_obsidian_alt_t_duplicate_tab.md`.

## Implementation Steps

1. Re-check vault state before editing:
   - `git -C /home/bryan/bob status --short`
   - targeted diffs for `.obsidian/plugins/bob-navigation-hotkeys/main.js` and `.obsidian/hotkeys.json`
2. Add the `duplicate-current-tab` command registration next to the existing tab commands.
3. Add `duplicateCurrentTab()` and any tiny private helpers needed for:
   - new-tab leaf creation
   - view-state cloning
   - focusing a leaf
   - optional adjacent placement after the source leaf
4. Add the `Alt+T` binding to `.obsidian/hotkeys.json`.
5. Run static validation:
   - `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js .obsidian/hotkeys.json`
6. Review the final live-vault diff and confirm it contains only the intended hunks in the two target files.
7. Manually smoke-test in Obsidian after reloading the plugin or app:
   - With a Markdown note active, `Alt+T` creates a second tab for the same note and focuses it.
   - Cursor/scroll/view state are preserved as closely as Obsidian's view state supports.
   - With reading view active, `Alt+T` duplicates the reading view rather than forcing edit mode.
   - With a non-Markdown tab active, the command works if the view supports view-state duplication; otherwise it fails
     with a concise notice and no data mutation.
   - Existing tab movement hotkeys still work.
8. Commit only this task's vault changes using the required SASE git commit workflow, leaving unrelated dirty notes
   untouched.

## Risks And Mitigations

- Obsidian workspace APIs are partly undocumented.
  - Mitigation: feature-detect every method, keep fallbacks narrow, and mirror the defensive style already used by
    `moveActiveTab`.
- `workspace.getLeaf("tab")` behavior could vary across Obsidian versions.
  - Mitigation: guard with try/catch and fallback to older leaf creation; validate in the live app.
- Some view types may not round-trip through `getViewState` / `setViewState`.
  - Mitigation: degrade only for active Markdown files and otherwise show a notice.
- Creating a target leaf before a later failure can leave a blank tab.
  - Mitigation: if implementation reveals a reliable `targetLeaf.detach()` cleanup path, use it on failure; otherwise
    keep fallback paths before showing the failure notice.
- Shared dirty vault state could accidentally be staged.
  - Mitigation: stage only `.obsidian/plugins/bob-navigation-hotkeys/main.js` and `.obsidian/hotkeys.json`, review
    `git diff --cached`, and leave unrelated notes untouched.

## Done Criteria

- `bob-navigation-hotkeys:duplicate-current-tab` exists and duplicates the active tab into a focused new tab.
- `Alt+T` is bound to that command in `.obsidian/hotkeys.json`.
- Static validation passes for the plugin and JSON config.
- Manual Obsidian smoke testing confirms Markdown tab duplication and no regression to existing tab movement hotkeys.
- The vault commit contains only the intended plugin and hotkey changes.
