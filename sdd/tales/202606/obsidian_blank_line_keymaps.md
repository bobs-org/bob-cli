---
title: Obsidian Vim Blank-Line Keymaps Plan
create_time: 2026-06-11 10:10:15
status: wip
prompt: sdd/prompts/202606/obsidian_blank_line_keymaps.md
---

# Obsidian Vim Blank-Line Keymaps (`[<Space>` / `]<Space>`) Plan

## Goal

Add vim-unimpaired-style normal-mode keymaps to Bryan's Bob Obsidian vault:

- `[<Space>` inserts a blank line **above** the current line.
- `]<Space>` inserts a blank line **below** the current line.

In both cases the cursor must stay on the original text line (at the same column), and the editor must remain in Vim
normal mode. The inserted line must be truly blank — no bullet/checkbox continuation prefixes.

This is a live-vault (`/home/bryan/bob`) change, not a `bob-cli` Rust/CLI change.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Long-term memory: the only Tier 2 file listed in `AGENTS.md` is `memory/long/cli_rules.md`, which is required only
  when adding CLI subcommands/options. This task adds none, so no audited long-memory read is required. (The
  `long/obsidian.md` file referenced by older plans no longer exists in this workspace's `memory/long/`.)
- Live vault rules: `/home/bryan/bob/AGENTS.md` — inspect `git status` before editing, never touch unrelated dirty
  files, and commit task-related vault changes with `/sase_git_commit` before finishing.
- Prior art:
  - `sase_plan_obsidian_vimrc_keymaps.md` — established the architecture: `obsidian_vimrc.md` is the declarative home
    for normal-mode key-to-command dispatch (`exmap` + `nmap`), while Bob's local plugins own the JavaScript command
    implementations.
  - `sase_plan_obsidian_vimrc_sync.md` — the active vimrc file is the Markdown note `/home/bryan/bob/obsidian_vimrc.md`
    (confirmed in `obsidian-vimrc-support/data.json`: `"vimrcFileName": "obsidian_vimrc.md"`); `.obsidian.vimrc` no
    longer exists.
  - `sdd/tales/202606/obsidian_vim_o_list_continuation.md` — `o` and `O` are custom Vim actions in the
    `task-status-cycler` plugin that continue list/checkbox prefixes.

## Current State (verified live)

- `/home/bryan/bob/obsidian_vimrc.md` currently defines `exmap` wrappers plus `nmap` mappings for `-`, `[[`, `]]`, and
  `!`, all dispatching to `obcommand` targets. JavaScript vimrc commands remain disabled (`supportJsCommands: false`).
- `bob-navigation-hotkeys/main.js` is the established home for `obcommand` targets, including the editing command
  `toggle-line-transclusions`, which uses `editorCallback: (editor) => ...`.
- `bob-navigation-hotkeys/main.js` already exports reusable editor helpers: `getEditorCursor`, `getEditorLine`,
  `replaceEditorLine`, and `setEditorCursorSafely` (all in `module.exports.helpers`).
- `task-status-cycler` maps `o`/`O` to custom list-continuation actions, so both keys insert `- ` / `- [ ] ` prefixes on
  list lines.
- Both target files (`obsidian_vimrc.md`, `bob-navigation-hotkeys/main.js`) are currently clean in the vault Git repo;
  the vault has many unrelated dirty/untracked files that must be left alone.

## Key Design Decisions

1. **Implement as Obsidian editor commands + vimrc dispatch, not as a pure vimrc key remap.**
   - A naive `nmap [<Space> O<Esc>`-style remap is wrong here: `nmap` in obsidian-vimrc-support is recursive, and
     `o`/`O` are overridden by `task-status-cycler` to continue list prefixes. The "blank" line would get `- ` or
     `- [ ] ` inserted, and the mapping would bounce through insert mode.
   - Instead, add two `editorCallback` commands to `bob-navigation-hotkeys` and dispatch them from `obsidian_vimrc.md`
     via the established `exmap` + `nmap :<excmd><CR>` pattern. This keeps the cursor and mode fully under our control.

2. **Put the commands in `bob-navigation-hotkeys`.**
   - It is already the home for every `obcommand` target in `obsidian_vimrc.md`, including the line-editing
     `toggle-line-transclusions` command, and it has the editor helpers needed for a safe implementation.
   - `task-status-cycler` was considered (it owns `o`/`O`), but it registers raw Vim actions, not Obsidian commands, and
     the migration direction is to keep key dispatch declarative in the vimrc file.

3. **Insertion semantics (vim-unimpaired behavior).**
   - Above: replace current line text with `"\n" + lineText`, then set the cursor to `(line + 1, ch)` so it stays on the
     original text at the original column.
   - Below: replace current line text with `lineText + "\n"`, then restore the cursor to `(line, ch)`.
   - Both reuse `replaceEditorLine` (which performs a `replaceRange` over the current line) and `setEditorCursorSafely`.
     Edge cases covered naturally: first line of file, last line of file (no trailing newline needed), and empty lines.

4. **Counts are out of scope.** `3[<Space>` will insert one blank line, not three. The `obcommand` dispatch path does
   not plumb Vim repeat counts (same limitation accepted in the earlier vimrc migration for ledger mappings). If count
   support is ever wanted, the fallback is a JS-registered Vim action, which is explicitly not this plan.

## Proposed Changes

### 1. `bob-navigation-hotkeys/main.js`

Add two commands in `onload()`, following the existing `toggle-line-transclusions` shape:

```js
this.addCommand({
  id: "insert-blank-line-above",
  name: "Insert blank line above",
  editorCallback: (editor) => this.insertBlankLine(editor, "above"),
});

this.addCommand({
  id: "insert-blank-line-below",
  name: "Insert blank line below",
  editorCallback: (editor) => this.insertBlankLine(editor, "below"),
});
```

Add one small method `insertBlankLine(cm, direction)` that:

1. Gets the cursor via `getEditorCursor`; on `null`, shows the existing "No active markdown editor" notice and returns
   `false`.
2. Reads the current line via `getEditorLine`.
3. Calls `replaceEditorLine` with `"\n" + lineText` (above) or `lineText + "\n"` (below).
4. Restores the cursor with `setEditorCursorSafely` — `(line + 1, ch)` for above, `(line, ch)` for below.

If any new pure helper is factored out (e.g., a function computing the replacement text and final cursor position),
export it through `module.exports.helpers` in the existing style so it is harness-testable.

### 2. `/home/bryan/bob/obsidian_vimrc.md`

Append, following the existing file's structure:

```vim
exmap bob_blank_line_above obcommand bob-navigation-hotkeys:insert-blank-line-above
exmap bob_blank_line_below obcommand bob-navigation-hotkeys:insert-blank-line-below

nmap [<Space> :bob_blank_line_above<CR>
nmap ]<Space> :bob_blank_line_below<CR>
```

Notes on `<Space>` notation feasibility (verified statically):

- The vimrc plugin parses mapping lines with `line.split(" ")`, so `[<Space>` and `]<Space>` survive tokenization as
  single LHS tokens (a literal space character could not be used, but the `<Space>` notation avoids that).
- The plugin's own settings UI documents chords rendered as `"<Space> f-"`, confirming `<Space>` is the normalized key
  name in Obsidian's CodeMirror Vim.
- The new mappings share the `[` / `]` prefix with the existing `[[` / `]]` mappings; CodeMirror Vim already handles
  multi-key prefix resolution for those today, so no new ambiguity is introduced.

## Files To Edit

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/obsidian_vimrc.md`

No expected edits:

- `task-status-cycler/main.js`, `bob-ledger-tools/main.js`, `obsidian-vimrc-support/*`, `.obsidian/hotkeys.json`
- Any `bob-cli` Rust/Python/CLI code, tests, or memory files
- Unrelated dirty/untracked vault content

## Implementation Steps

1. Re-check vault state: `git -C /home/bryan/bob status --short` and confirm the two target files are still clean.
2. Add the `insertBlankLine` method and the two `addCommand` entries to `bob-navigation-hotkeys/main.js`; export any new
   pure helper via `module.exports.helpers`.
3. Append the `exmap`/`nmap` lines to `/home/bryan/bob/obsidian_vimrc.md`.
4. Validate statically:
   - `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `git -C /home/bryan/bob diff --check -- obsidian_vimrc.md .obsidian/plugins/bob-navigation-hotkeys/main.js`
5. Focused behavior checks with a lightweight Node harness (stubbed `obsidian` module + fake editor object, per the
   established local-plugin test pattern), asserting:
   - Above on line N: a blank line appears at N, cursor ends at `(N + 1, ch)` on the original text.
   - Below on line N: a blank line appears at N + 1, cursor stays at `(N, ch)`.
   - First line of file (above) and last line of file (below) both work.
   - The current line's own text (including list/task prefixes) is unchanged, and the new line is empty.
6. Manual Obsidian smoke test after reloading the plugin/Obsidian, in Vim normal mode:
   - `[<Space>` on a mid-file line inserts a blank line above; cursor stays put on the same text/column; still normal
     mode.
   - `]<Space>` inserts a blank line below with the same cursor/mode behavior.
   - On a `- [ ]` task line, the inserted lines are truly blank (no list continuation).
   - First/last line of a note behave correctly.
   - Existing mappings still work: `[[`, `]]`, `-`, `!`, `o`/`O` list continuation, and the `\`-prefixed ledger maps.
   - In insert mode, typing `[` or `]` followed by a space inserts literal text.
7. Git hygiene and commit:
   - Confirm the vault diff is limited to the two target files.
   - Commit only those files with the `/sase_git_commit` workflow before finishing (required by
     `/home/bryan/bob/AGENTS.md`).

## Risks And Mitigations

- **Risk:** `<Space>` in the `nmap` LHS fails to register in the live CodeMirror Vim despite the static evidence.
  **Mitigation:** verify live during the smoke test. Fallback: register the two mappings in JavaScript via
  `vim.defineAction` + `vim.mapCommand` in `bob-navigation-hotkeys` (the same pattern `task-status-cycler` uses),
  keeping the Obsidian commands as the behavior owners either way.
- **Risk:** `obcommand` → `editorCallback` requires an active markdown editor and might no-op in preview mode or
  non-markdown views. **Mitigation:** this matches the existing `!` mapping's behavior; the `getEditorCursor` null guard
  plus notice keeps failure graceful.
- **Risk:** the vault's many unrelated dirty files obscure the task diff. **Mitigation:** targeted `git status`/`diff`
  on the two files before and after edits; stage and commit only those files.
- **Risk:** cursor drift after the above-insertion if the editor auto-maps positions through the change. **Mitigation:**
  always set the cursor explicitly via `setEditorCursorSafely` after the edit rather than relying on position mapping.

## Done Criteria

- `[<Space>` and `]<Space>` work in Vim normal mode in the live vault, inserting truly blank lines above/below while
  keeping the cursor on the original text line and staying in normal mode.
- The new behavior is dispatched declaratively from `obsidian_vimrc.md` to `bob-navigation-hotkeys` commands (also
  invokable from the command palette).
- Static validation and the Node harness checks pass.
- All pre-existing keymaps continue to work.
- The committed vault diff contains only the two task files, committed via the SASE git commit workflow.
