---
create_time: 2026-06-19 07:40:09
status: done
prompt: sdd/prompts/202606/obsidian_open_task_jumps.md
---
# Plan: Obsidian Ctrl+Shift+J/K Open-Task Navigation

## Goal

Add Obsidian keymaps in Bryan's Bob vault so:

- `<Ctrl+Shift+J>` jumps to the line containing the next open Obsidian task in the current note file.
- `<Ctrl+Shift+K>` jumps to the line containing the previous open Obsidian task in the current note file.

The behavior should mirror the existing `<Ctrl+J>` / `<Ctrl+K>` section-header navigation where appropriate: same local
navigation plugin, same editor-command shape, same vimrc plus `hotkeys.json` coverage, and the same top-of-viewport
scroll after a successful jump.

## Context Reviewed

- Required Obsidian long-term memory was read through:
  `sase memory read obsidian.md --reason "Need Obsidian workflow context before planning keymap behavior for open-task navigation"`.
- This task changes the live Obsidian vault at `/home/bryan/bob`, not the Rust `bob-cli` command implementation. No new
  CLI subcommands or CLI options are planned, so `memory/long/cli_rules.md` does not apply.
- Vault rules from `/home/bryan/bob/AGENTS.md`: inspect vault git status before editing, preserve unrelated dirty files,
  and commit only task-related vault changes with `/sase_git_commit` before terminating after any implementation edits
  under `~/bob`.
- Current vault status has pre-existing dirty note files and a pre-existing `obsidian_vimrc.md` diff: `\d<`, `\d>`,
  `\do` are already changed to `\<`, `\>`, `\\`. That vimrc change must be preserved and not treated as part of this
  task except for the new task-jump lines that may be added nearby.
- `~/bob/.obsidian/plugins/obsidian-vimrc-support/data.json` points at the vault-root `obsidian_vimrc.md`; there is no
  separate `.obsidian.vimrc` target for this setup.
- `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json` has `globalFilter: "#task"` and `taskFormat: "dataview"`.
- Existing vault task conventions define open Tasks-plugin statuses as `[ ]`, `[/]`, and `[B]`; `[x]`/`[X]` are done and
  `[-]` is canceled.
- Existing `<Ctrl+J>` / `<Ctrl+K>` section-header navigation is implemented in
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`, mapped in `obsidian_vimrc.md`, and bound in
  `.obsidian/hotkeys.json`.
- `bob-navigation-hotkeys/main.js` already has the useful scanning and editor primitives: `startsWithFrontmatter`,
  `getFenceOpening`, `isClosingFence`, `getSectionHeaderJumpLine`, `setEditorCursor`, and `scrollEditorLineToTop`.
- `hotkeys.json` currently has no `Ctrl+Shift+J` or `Ctrl+Shift+K` vault bindings.

## Product Decisions

1. A target is a "proper Obsidian task" in the Bob vault sense:
   - a Markdown checkbox list item, including indented, ordered, unordered, and blockquote-prefixed task lines;
   - carrying a standalone `#task` token, matching the Tasks plugin global filter;
   - with an open status symbol: space, `/`, or `B`.

2. Plain checklists without `#task` are deliberately ignored. This avoids jumping through incidental checklists that the
   Tasks plugin itself would not treat as Bob tasks.

3. Done and canceled tasks are ignored. This includes `[x]`, `[X]`, and `[-]`.

4. Task lines inside YAML frontmatter or fenced code blocks are ignored, matching the existing header-jump scanner and
   avoiding false positives in examples, `tasks` query blocks, `dataview` blocks, and Markdown snippets.

5. Jumps are strict and non-wrapping:
   - next means the nearest matching task line strictly below the current cursor line;
   - previous means the nearest matching task line strictly above the current cursor line;
   - if the cursor is already on an open task, the command jumps to the following/preceding open task, not itself.

6. The cursor lands at column 0 of the target line. After the cursor moves, reuse `scrollEditorLineToTop` so the task
   line is redrawn at the top of the viewport, matching the later header-jump `zt` behavior.

7. No-target cases leave the cursor and scroll position unchanged and show a notice:
   - `No next open task`
   - `No previous open task`

8. Reading mode is out of scope. Source mode and live preview editor panes are in scope via Obsidian editor commands.

9. Vim counts are out of scope for this feature, consistent with the existing vimrc `obcommand` mappings for header
   navigation.

## Implementation Approach

### 1. Add pure task-scanning helpers in `bob-navigation-hotkeys`

Edit `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

Add generic task-navigation constants near the existing navigation constants, without changing the existing
project-specific parser behavior:

- `OPEN_OBSIDIAN_TASK_STATUSES = new Set([" ", "/", "B"])`
- an Obsidian task line regex based on the broader `task-status-cycler` parser shape, supporting indentation, blockquote
  prefixes, unordered markers, and ordered markers;
- a standalone `#task` boundary regex equivalent to the existing project/task boundary behavior.

Add exported pure helpers:

- `isOpenObsidianTaskLine(lineText)`
  - returns true only for checkbox list items with standalone `#task` and an open status.
- `getOpenObsidianTaskLines(lines)`
  - normalizes string or array input;
  - scans top-down;
  - skips leading frontmatter and fenced code blocks with the same state-machine helpers used for headers;
  - returns matching zero-based line indices.
- `getOpenObsidianTaskJumpLine(lines, cursorLine, direction)`
  - mirrors `getSectionHeaderJumpLine`;
  - returns the first matching line greater than the cursor line for direction `1`;
  - returns the last matching line less than the cursor line for direction `-1`;
  - returns `null` for invalid cursor input or no target.

Export these helpers in `module.exports.helpers` for focused Node tests.

### 2. Add editor commands

In `BobNavigationHotkeysPlugin.onload()`, register:

- `jump-to-next-open-task` / `Jump to next open task`
- `jump-to-prev-open-task` / `Jump to previous open task`

Both should use `editorCallback` and call a new method:

- `jumpToOpenObsidianTask(editor, direction)`

The method should follow `jumpToSectionHeader` closely:

1. Read the current editor cursor and current buffer text.
2. Compute the target with `getOpenObsidianTaskJumpLine`.
3. On no target, show the direction-specific notice and return `false`.
4. On success, call `setEditorCursor(editor, { line: targetLine, ch: 0 })`.
5. Call `scrollEditorLineToTop(editor, targetLine)` as best-effort follow-up.
6. Return `true`.

### 3. Add Obsidian hotkeys for insert-mode and non-vim coverage

Edit `/home/bryan/bob/.obsidian/hotkeys.json` narrowly:

```json
"bob-navigation-hotkeys:jump-to-next-open-task": [
  { "modifiers": ["Ctrl", "Shift"], "key": "J" }
],
"bob-navigation-hotkeys:jump-to-prev-open-task": [
  { "modifiers": ["Ctrl", "Shift"], "key": "K" }
]
```

Preserve all existing bindings and formatting as much as practical. No existing vault binding currently uses these two
chords.

### 4. Add vim normal-mode mappings

Edit `/home/bryan/bob/obsidian_vimrc.md` carefully, preserving the pre-existing unrelated diff.

Add exmaps near the existing header exmaps:

```vim
exmap bob_next_open_task obcommand bob-navigation-hotkeys:jump-to-next-open-task
exmap bob_prev_open_task obcommand bob-navigation-hotkeys:jump-to-prev-open-task
```

Add nmaps near the existing `<C-j>` / `<C-k>` header mappings:

```vim
nmap <C-S-j> :bob_next_open_task<CR>
nmap <C-S-k> :bob_prev_open_task<CR>
```

Important implementation check: confirm in live Obsidian or with the installed vimrc support behavior that `<C-S-j>` and
`<C-S-k>` remain distinct from the existing `<C-j>` and `<C-k>` header maps. If CodeMirror Vim collapses
Ctrl+Shift+letter into Ctrl+letter, do not overwrite the existing header mappings. Instead, add a narrow capture-phase
fallback in `bob-navigation-hotkeys` similar to `task-status-cycler`'s Ctrl+Shift+O fallback:

- detect exactly `ctrlKey && shiftKey && !altKey && !metaKey`;
- require `event.code` to be `KeyJ` or `KeyK`;
- require the event target to be inside the active Markdown editor;
- require Vim normal mode before intercepting, using the same practical detection pattern already present in
  `task-status-cycler`;
- prevent default and dispatch the appropriate task-jump command.

The preferred final state is the simple vimrc mapping if it works. The fallback exists only to keep `Ctrl+Shift+J/K`
reliable without sacrificing the existing `Ctrl+J/K` header jumps.

## Expected Files Changed After Implementation

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/obsidian_vimrc.md`

No expected edits:

- `bob-cli` Rust code, tests, README, or memory files
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/.obsidian/plugins/obsidian-tasks-plugin/*`
- `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/*`
- vault notes, except for a temporary scratch note only if manual testing needs one and it is cleaned up before finish

## Validation Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
jq '.' /home/bryan/bob/.obsidian/hotkeys.json
jq '.' /home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/hotkeys.json \
  obsidian_vimrc.md
```

Focused Node helper tests with stubbed `obsidian` and `@codemirror/view` modules:

- `isOpenObsidianTaskLine` returns true for:
  - `- [ ] #task Follow up`
  - `- [/] #task In progress`
  - `- [B] #task Blocked`
  - `  1. [ ] #task Ordered nested`
  - `> - [ ] #task Quoted task`
- It returns false for:
  - `- [x] #task Done`
  - `- [X] #task Done uppercase`
  - `- [-] #task Canceled`
  - `- [ ] Checklist without global filter`
  - `- [ ] #taskish Not standalone`
  - non-list prose.
- `getOpenObsidianTaskLines` skips task-looking lines in leading frontmatter and fenced code blocks, including backtick
  and tilde fences.
- `getOpenObsidianTaskJumpLine` chooses the nearest strict next/previous match and returns `null` at boundaries.
- `jumpToOpenObsidianTask` moves to `{ line: targetLine, ch: 0 }`, calls the top-scroll helper on success, and leaves
  the cursor untouched on no-target cases.

Configuration checks:

- Confirm `hotkeys.json` contains the two new command IDs with `["Ctrl", "Shift"]` and uppercase `J`/`K`.
- Confirm `obsidian_vimrc.md` contains the new exmaps and normal-mode maps without removing the existing `<C-j>` and
  `<C-k>` header maps or the pre-existing close-tab changes.

Manual Obsidian smoke test after reloading Obsidian or toggling `bob-navigation-hotkeys`:

1. In a scratch note with multiple open `#task` tasks, done tasks, canceled tasks, ordinary checklists, frontmatter, and
   fenced examples, press `<Ctrl+Shift+J>` repeatedly from normal mode. It should visit only open `#task` lines below.
2. Press `<Ctrl+Shift+K>` repeatedly. It should visit only open `#task` lines above.
3. Confirm `<Ctrl+J>` and `<Ctrl+K>` still navigate section headers.
4. In insert mode, confirm the same `Ctrl+Shift+J/K` commands fire through `hotkeys.json`.
5. Confirm no-target cases show the appropriate notice and do not move the cursor.
6. If the vimrc mapping path cannot distinguish shifted control letters, confirm the fallback listener handles normal
   mode while insert mode still uses Obsidian hotkeys.

Vault hygiene:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/hotkeys.json \
  obsidian_vimrc.md
```

Review the final diff to ensure only intended task-related hunks are present. If implementation edits are made under
`~/bob`, commit only those task-related vault files with `/sase_git_commit` before finishing, leaving pre-existing dirty
notes and unrelated vimrc hunks untouched.

## Risks And Mitigations

- **Ctrl+Shift letter handling in Vim mode:** CodeMirror Vim may collapse shifted control letters into the unshifted
  control mappings. Mitigation: test before relying on vimrc-only normal-mode coverage; if necessary, use a narrow
  capture-phase fallback that preserves existing `<Ctrl+J>/<Ctrl+K>` header navigation.
- **Task definition drift:** The Tasks plugin can support configurable custom statuses. Current vault convention and
  docs only treat space, `/`, and `B` as open. Mitigation: keep the status set explicit and covered by tests; future
  custom statuses can be added by changing one constant.
- **False positives in examples:** Markdown examples and Tasks query blocks can contain task-shaped lines. Mitigation:
  reuse the existing frontmatter and fenced-code skip machinery.
- **Unrelated vault sync changes:** The vault is dirty before this task. Mitigation: inspect status and diffs before
  editing, patch only target hunks, and stage/commit only current-task files through the required SASE commit workflow
  if implementation proceeds.
