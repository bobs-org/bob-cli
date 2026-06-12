---
create_time: 2026-06-12 14:21:11
status: done
prompt: sdd/prompts/202606/obsidian_move_tab_keymaps.md
---
# Obsidian Move-Tab Keymaps (`Ctrl+Shift+,` / `Ctrl+Shift+.`)

## Goal

Add two new global Obsidian keymaps to Bryan's Bob vault:

- `Ctrl+Shift+,` moves the active tab one position to the LEFT within its tab group.
- `Ctrl+Shift+.` moves the active tab one position to the RIGHT within its tab group.

Focus must stay on the moved tab, and the keymaps should work regardless of Vim mode or tab content type (Markdown, PDF,
canvas, etc.).

## Context Reviewed

- Obsidian core has NO command for reordering tabs within a tab group. This was verified against current Obsidian forum
  feature requests (e.g. "Move / Change position of tabs with commands (within the same tab group)",
  forum.obsidian.md/t/103109) — drag-and-drop is the only core mechanism. The accepted community answer is the Pane
  Relief plugin's "swap tab" commands.
- Pane Relief's implementation (github.com/pjeby/pane-relief, `src/pane-relief.ts`, `leafPlacer`) grounds the technical
  approach: splice the leaf within `parent.children`, then call `parent.selectTab(leaf)` on `WorkspaceTabs` (with a
  DOM-reorder + `recomputeChildrenDimensions()` + `setActiveLeaf` fallback for non-tab parents).
- Vault keymap surfaces (from prior keymap tasks in this repo):
  - `/home/bryan/bob/.obsidian/hotkeys.json` — global Obsidian hotkeys (currently clean in git).
  - `/home/bryan/bob/obsidian_vimrc.md` — Vim normal-mode command dispatch via obsidian-vimrc-support.
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` — the bespoke plugin that owns all custom
    navigation commands (single hand-maintained file, no build step).
- Existing hotkey conventions: `Ctrl+,` = open-alt-file-note, `Ctrl+.` = open-template-note, and several
  `Ctrl+Shift+<letter>` bob commands exist. `Ctrl+Shift+,` and `Ctrl+Shift+.` are unbound in `hotkeys.json` and have no
  Obsidian defaults (`app:open-settings` is explicitly unbound), so there are no conflicts.
- Vault state: `/home/bryan/bob` is actively synced and dirty. Critically, `bob-navigation-hotkeys/main.js` itself has
  ~85 lines of unrelated uncommitted changes (a dash-tasks scroll-assert feature). Per `/home/bryan/bob/AGENTS.md`, only
  task-related changes may be staged/committed, and any vault edits must be committed via the `/sase_git_commit`
  workflow before the agent terminates.
- Tier-2 memory check: `memory/long/cli_rules.md` triggers only for new CLI subcommands/options. This task changes no
  bob-cli code, so it does not apply.

## Key Product Decisions

1. Implement the commands in `bob-navigation-hotkeys`, not by installing Pane Relief.
   - House style: every prior keymap task added commands to the bespoke bob plugins; all custom navigation behavior
     already lives in `bob-navigation-hotkeys`.
   - Pane Relief bundles many unrelated features (per-pane history, focus lock, numbered tab jumps) and adds third-party
     code plus cross-device plugin-sync surface for what is ~40 lines using the same internals.

2. Bind the keys globally in `hotkeys.json`, not in `obsidian_vimrc.md`.
   - Global hotkeys fire in any Vim mode, in reading view, and on non-editor tabs (PDFs, canvases) — tab movement is a
     workspace operation, not an editor operation.
   - This matches the sibling bindings `Ctrl+,` and `Ctrl+.`, making the Shift variants thematically adjacent.
   - Use explicit `"Ctrl"` (not `"Mod"`) + `"Shift"` modifiers, matching every other bob-navigation-hotkeys binding.

3. Edge behavior: clamp (no-op) at the first/last tab position, matching Pane Relief. No wrap-around.

4. Match the plugin's defensive feature-detection style (`typeof parent.selectTab === "function"` guards), no-op
   gracefully when there is no active leaf, no parent tab container, or fewer than two tabs.

## Technical Design

New commands in `bob-navigation-hotkeys/main.js`:

- `move-tab-left` ("Move tab left") and `move-tab-right` ("Move tab right"), both `callback` commands delegating to a
  shared `moveActiveTab(offset)` method/helper with `offset` of `-1` / `+1`.

`moveActiveTab(offset)` behavior (mirroring Pane Relief's proven `leafPlacer`):

1. `leaf = this.app.workspace.activeLeaf`; return if falsy.
2. `parent = leaf.parent || leaf.parentSplit`; return if falsy or `parent.children` is not an array.
3. `fromPos = parent.children.indexOf(leaf)`; return if `-1`.
4. `toPos = fromPos + offset`; return (no-op) if out of `[0, children.length)`.
5. Splice the leaf out of `children` at `fromPos` and back in at `toPos`.
6. If `typeof parent.selectTab === "function"`, call `parent.selectTab(leaf)` — this re-renders the tab header strip and
   keeps the moved tab active (the normal path: tab-group parents are `WorkspaceTabs`).
7. Otherwise fall back to Pane Relief's non-tabs path: reposition `leaf.containerEl` relative to the displaced sibling,
   call `parent.recomputeChildrenDimensions()` / `leaf.onResize()` / `workspace.onLayoutChange()` when available, and
   re-focus via `workspace.setActiveLeaf(leaf, { focus: true })`.

New `hotkeys.json` entries (preserving the file's existing formatting/order conventions):

```json
"bob-navigation-hotkeys:move-tab-left": [
  { "modifiers": ["Ctrl", "Shift"], "key": "," }
],
"bob-navigation-hotkeys:move-tab-right": [
  { "modifiers": ["Ctrl", "Shift"], "key": "." }
]
```

## Implementation Scope

Files to edit (vault only — no bob-cli code changes):

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`

No expected edits: `obsidian_vimrc.md`, plugin `manifest.json`, `community-plugins.json`, other plugins, notes, memory
files, or anything in the bob-cli workspace.

## Implementation Steps

1. Re-check vault state: `git -C /home/bryan/bob status --short` and a targeted diff of
   `bob-navigation-hotkeys/main.js`. Confirm whether the unrelated dash-tasks scroll-assert hunks are still uncommitted.
2. Add the `moveActiveTab` helper and the two `addCommand` registrations to `main.js`, alongside the existing command
   block (after `delete-current-file` / near the other workspace-level commands), following the file's existing naming
   and guard conventions.
3. Add the two bindings to `hotkeys.json`, editing minimally so the diff touches only the new entries.
4. Static validation:
   - `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
   - `git -C /home/bryan/bob diff -- .obsidian/hotkeys.json .obsidian/plugins/bob-navigation-hotkeys/main.js` to confirm
     the new hunks are exactly the intended ones.
5. Manual Obsidian smoke test after an app/plugin reload, with at least three tabs open:
   - `Ctrl+Shift+.` moves the active tab one slot right; `Ctrl+Shift+,` moves it one slot left.
   - Focus stays on the moved tab; tab header order updates immediately.
   - No-op (no error, no notice spam) at the leftmost/rightmost position and when only one tab is open.
   - Works in Vim normal AND insert mode, in reading view, and on a non-Markdown tab (e.g. a PDF).
   - Existing `Ctrl+,` / `Ctrl+.` bindings still work unchanged.
6. Commit hygiene (the delicate part): `main.js` may still contain unrelated pre-existing hunks.
   - If the unrelated dash-tasks changes have been committed by their owning task by implementation time, stage and
     commit both task files normally.
   - If unrelated hunks remain in `main.js`, stage ONLY this task's hunks non-interactively (generate a focused patch of
     the new helper/commands and `git apply --cached` it, or equivalent), verify `git diff --cached` contains only
     tab-move changes plus the `hotkeys.json` entries, and leave the unrelated worktree changes untouched.
   - Commit via the required `/sase_git_commit` workflow before terminating, per `/home/bryan/bob/AGENTS.md`. All other
     dirty/untracked vault files stay untouched.

## Risks And Mitigations

- Undocumented workspace internals (`parent.children`, `selectTab`, `recomputeChildrenDimensions`) could change in a
  future Obsidian release. Mitigation: feature-detect every call and no-op on absence — the same exposure Pane Relief
  has carried for years; vault `minAppVersion` is 1.8.7 where these are known-good.
- `app.workspace.activeLeaf` is soft-deprecated but remains functional and is what Pane Relief uses; guard for null and
  accept the deprecation (consistent with reading the leaf, not opening files).
- Shared-dirty-file staging: committing `main.js` wholesale would smuggle in the unrelated dash-tasks work. Mitigation:
  the explicit partial-staging step above, plus a staged-diff review before committing.
- macOS device: explicit `"Ctrl"` stays the literal Control key on the MacBook, consistent with all existing
  bob-navigation hotkeys; no `"Mod"`/Cmd ambiguity is introduced.
- Sync propagation: `hotkeys.json` syncs with vault configuration; plugin `main.js` propagates only where community
  plugin sync is enabled — identical surface to every prior bob keymap change, so no new sync requirements.
- No automated test harness exists for the plugin's workspace-level behavior (the `module.exports` helpers are for
  pure-function checks only); tab movement requires the Obsidian runtime, so verification is the manual smoke test,
  matching prior keymap tasks.

## Done Criteria

- `bob-navigation-hotkeys:move-tab-left` / `move-tab-right` commands exist and behave per the design (clamped,
  focus-preserving, feature-detected).
- `Ctrl+Shift+,` / `Ctrl+Shift+.` are bound in `hotkeys.json` and pass the manual smoke test on the live vault.
- Static validation passes (`node --check`, `jq`).
- The committed vault diff contains ONLY the tab-move changes to `main.js` and the two new `hotkeys.json` entries,
  committed via `/sase_git_commit`; all unrelated pre-existing vault changes are preserved uncommitted.
