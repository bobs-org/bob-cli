---
create_time: 2026-06-12 12:46:12
status: wip
prompt: sdd/prompts/202606/obsidian_backslash_pipe_dash_tasks.md
---
# Plan: `\|` Keymap — Focus `dash.md`, Jump to `## Tasks`, Redraw at Top (`zt`)

## Context

Bryan wants a new vim normal-mode keymap `\|` (backslash, then pipe) in his Obsidian vault (`~/bob`) that:

1. focuses `~/bob/dash.md` (the "Dashboard" note),
2. moves the cursor to its `## Tasks` line (currently line 10, but found dynamically), and
3. redraws the editor with that line at the **top** of the viewport — vim's `zt` behavior.

Relevant facts from the current vault setup:

- Vim keymaps live in `~/bob/obsidian_vimrc.md`, loaded by obsidian-vimrc-support (`vimrcFileName: "obsidian_vimrc.md"`,
  `supportJsCommands: false`). The established pattern is an `exmap bob_x obcommand <plugin>:<command>` line plus an
  `nmap <keys> :bob_x<CR>` line — e.g. `-` → `daily-notes`, `<C-j>`/`<C-k>` → the `bob-navigation-hotkeys` header-jump
  commands. Since JS vimrc commands are disabled, the keymap must route through a registered Obsidian command.
- obsidian-vimrc-support feeds each vimrc line to CodeMirror Vim's `handleEx` (main.js:799), which splits map arguments
  on whitespace only — `|` has no special meaning there, so `nmap \| :bob_dash_tasks<CR>` registers the literal two-key
  sequence `\` then `|`. This is the same mechanism behind the working multi-key `\`-prefix maps (`\\`, `\p`, `\P`,
  `\o`, `\O`) that `bob-ledger-tools` registers programmatically (main.js:1553-1567). None of those collide with `\|`.
- The local plugin `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` already owns the building blocks:
  - `scrollEditorLineToTop(editor, line)` (main.js:1527) — the CM6 `EditorView.scrollIntoView(offset, {y: "start"})`
    `zt` helper built for the `<Ctrl+J>/<Ctrl+K>` header jumps, with graceful `false` return when CM6 is unavailable.
  - `setEditorCursor(editor, position)` (main.js:1501) — cursor set + centered scroll; `jumpToSectionHeader`
    (main.js:3303) shows the exact sequence to copy: `setEditorCursor` first, `scrollEditorLineToTop` second, so the
    top-align dispatch wins and centering remains the fallback.
  - Fence-aware line scanning helpers (`getFenceOpening` main.js:1261, `isClosingFence` main.js:1273) so a header match
    inside a code fence can be skipped.
  - File-open conventions: `captureActiveFilePosition()` before leaving a file, `getLeaf(false).openFile(file)` to open
    in the current leaf, and a `deferToNextFrame`/`cancelDeferred` retry pattern (`deferRestoreFilePosition`,
    main.js:4430) for acting on an editor that is not attached yet.
  - Pure/editor helpers exported via `module.exports.helpers` (main.js:4926) for the established ad-hoc Node test
    pattern (stub `obsidian` and `@codemirror/view` modules).
- `bob-ledger-tools` (main.js:1167-1236) holds the proven tab-reuse pattern from the `\\` daily-note work: scan
  `workspace.iterateAllLeaves` for a markdown leaf whose `view.file.path` matches, activate it via
  `workspace.revealLeaf` (fallback `setActiveLeaf(leaf, {focus: true})`), then wait for the active markdown view before
  touching the editor. Bryan explicitly preferred reusing an already-open tab over opening a duplicate.
- `~/bob/dash.md` exists at the vault root with `## Tasks` at line 10; its other headings (`## Projects ...`,
  `## Reading List ...`) and a ` ```tasks ` code fence mean the header search must match exactly and skip fences.
- Vault discipline (`~/bob/AGENTS.md`): inspect `git status` first, never touch unrelated dirty files, stage/commit only
  task files via `/sase_git_commit`. The vault currently has unrelated dirty/untracked files — including `dash.md`
  itself — that must be left untouched. The two files this task changes (`obsidian_vimrc.md`,
  `.obsidian/plugins/bob-navigation-hotkeys/main.js`) are currently clean.
- No bob-cli (Rust) changes are involved; this is a vault-only change, so the Tier 2 `cli_rules.md` memory (CLI
  subcommands) does not apply.

## Goal

Pressing `\|` in vim normal mode anywhere in the vault focuses `dash.md` and lands the cursor on the `## Tasks` line
with that line redrawn at the top of the viewport, exactly like jumping there and pressing `zt`.

## Behavior Specification

1. **From any note (dash not open anywhere):** `\|` opens `dash.md` in the current leaf (matching the plugin's other
   open commands), places the cursor at column 0 of the `## Tasks` line, and scrolls that line to the top of the
   viewport (CM6's default small `yMargin` keeps it from clipping).
2. **Dash already open in another tab/pane:** `\|` activates that existing leaf instead of opening a duplicate, then
   performs the same jump + top-scroll there.
3. **Dash is already the active file:** no reopen/re-focus churn — just jump to `## Tasks` and top-scroll (pressing `\|`
   while reading the bottom of the dashboard snaps back to the Tasks section).
4. **Header matching:** first line whose trimmed text is exactly `## Tasks`, skipping lines inside fenced code blocks.
   If no such line exists, show a Notice (e.g. `No "## Tasks" header in dash.md`); the file stays focused, cursor
   untouched.
5. **Missing file:** if `dash.md` does not exist, show a Notice (`dash.md not found`) and do nothing else — the
   dashboard is a curated note; silently creating an empty one would be wrong.
6. **Graceful degradation:** if the CM6 view is unavailable, the jump still happens with `setEditorCursor`'s centered
   scroll (same contract as the header-jump `zt` work). A failed top-scroll is never treated as a failed jump.
7. **Scroll-restore races:** the jump runs only after dash's editor is the active markdown view (immediate attempt plus
   deferred retries), so it wins over Obsidian's own remembered scroll/cursor state for the file.
8. Existing commands, keymaps, and the cursor-position-restore/alternate-file features keep their current behavior; the
   `\`-prefix ledger maps are untouched.

## Implementation

Two files change, both in the vault (no changes in the bob-cli repo):

### 1. `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` — new `open-dash-tasks` command

1. Constants: `DASH_FILE_PATH = "dash.md"`, `DASH_TASKS_HEADER = "## Tasks"`.
2. Pure helper `getDashTasksHeaderLine(lines)` next to the other line-scanning helpers: walk the lines tracking fence
   state with the existing `getFenceOpening`/`isClosingFence` helpers, return the 0-based index of the first
   out-of-fence line whose trimmed text equals `## Tasks`, else `null`. Export it via `module.exports.helpers`.
3. Plugin method `openDashTasks()`:
   - Resolve `this.app.vault.getAbstractFileByPath(DASH_FILE_PATH)`; if not a markdown file (`isMarkdownFile`), Notice
     `dash.md not found` and stop.
   - If the active markdown view already shows dash, jump immediately (step below) — no reopen.
   - Otherwise `captureActiveFilePosition()` (keeps alternate-file/cursor-restore bookkeeping coherent), then:
     - scan `workspace.iterateAllLeaves` for a markdown leaf whose `view.file.path` is `dash.md` (defensive feature
       checks, mirroring the bob-ledger-tools helper); if found, activate via `revealLeaf` with
       `setActiveLeaf(leaf, {focus: true})` fallback;
     - else `await this.app.workspace.getLeaf(false).openFile(file)`.
   - Jump step: read the active dash editor; if it is not attached yet, retry on subsequent frames using the existing
     `deferToNextFrame`/`cancelDeferred` machinery (a small bounded retry helper modeled on `deferRestoreFilePosition`,
     with a tracked pending handle cleared in `onunload`). Once available: compute the target line with
     `getDashTasksHeaderLine(editor.getValue().split(/\r?\n/))`; if `null`, Notice `No "## Tasks" header in dash.md`;
     otherwise `setEditorCursor(editor, {line, ch: 0})` followed by `scrollEditorLineToTop(editor, line)` — the proven
     `jumpToSectionHeader` sequence where the `y: "start"` dispatch supersedes the centered scroll.
4. Register the command alongside the others (main.js:2996+):
   `this.addCommand({ id: "open-dash-tasks", name: "Open dash Tasks section", callback: () => this.openDashTasks() })`.
   A plain `callback` (not `editorCallback`) because the command changes focus rather than operating on the current
   editor; it also becomes palette-invocable for free, and the vimrc `obcommand` dispatcher executes plain callbacks
   directly.

### 2. `~/bob/obsidian_vimrc.md` — the keymap

Append, following the existing exmap/nmap pattern:

```
exmap bob_dash_tasks obcommand bob-navigation-hotkeys:open-dash-tasks
nmap \| :bob_dash_tasks<CR>
```

No changes to `hotkeys.json` (this is a vim-only chord), plugin `manifest.json`, or any other vault file.

**Contingency:** if live testing shows CodeMirror Vim mishandles the `\|` LHS from the vimrc (not expected — `handleEx`
map args are whitespace-split and key matching is per-character), fall back to registering the map programmatically in
the plugin via `vim.defineAction` + `vim.mapCommand("\\|", ...)`, the exact mechanism bob-ledger-tools uses for its
`\`-prefix maps, and drop the nmap line (keeping the exmap/command for palette use).

## Validation

1. `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
2. Ad-hoc Node test (temp dir, stub `obsidian` and `@codemirror/view` modules — the plugin's established pattern)
   exercising `helpers.getDashTasksHeaderLine`:
   - returns the index of `## Tasks` in a dash-like fixture (frontmatter + `# Dash` + `## Tasks` + fenced ` ```tasks `
     block + later `## Projects`);
   - skips a `## Tasks` line that only appears inside a code fence;
   - returns `null` when the header is absent;
   - tolerates trailing whitespace on the header line;
   - plus a quick regression that existing helper exports (e.g. `scrollEditorLineToTop`) still load.
3. Re-read the vimrc diff: only the two appended lines; confirm no existing `nmap` uses a `\`-prefixed LHS that `\|`
   could shadow.
4. `git -C /home/bryan/bob status --short` before and after: only `obsidian_vimrc.md` and
   `.obsidian/plugins/bob-navigation-hotkeys/main.js` newly modified; the pre-existing dirty files (including `dash.md`)
   untouched.
5. Commit only those two files via `/sase_git_commit`.

## Manual Smoke Test (after reloading Obsidian / toggling the plugin)

1. From a non-dash note with dash.md closed: `\|` opens dash.md in the current tab, cursor on `## Tasks`, that line
   redrawn at the top of the viewport (not centered).
2. With dash.md open in another tab: `\|` switches to that tab (no duplicate dash tab) and performs the same jump +
   top-scroll.
3. While already in dash.md scrolled to the bottom: `\|` snaps the cursor and viewport back to `## Tasks` at the top.
4. Confirm the ledger maps still work (`\\` pomodoro jump, `\p` add unit) — no prefix interference.
5. Command palette: "Open dash Tasks section" performs the same behavior.
6. Temporarily rename the `## Tasks` header in a scratch copy scenario (or test the Notice path via the Node test) —
   Notice appears, no cursor jump.

## Risks

- **`\|` LHS parsing:** pipe could theoretically be mishandled somewhere in the ex-command path. Mitigated by the
  documented contingency (programmatic `vim.mapCommand` registration, the proven `\`-prefix mechanism).
- **Editor not attached after leaf activation/open:** mitigated by the bounded next-frame retry before jumping, the same
  defense the cursor-restore feature uses.
- **Obsidian's remembered scroll state racing the `zt` scroll:** mitigated by jumping only after the dash view is
  active; the `y: "start"` dispatch is the last scroll effect issued.
- **Vault dirty-file discipline:** `dash.md` itself is dirty with unrelated user/sync changes — this task must never
  stage it; only the plugin file and the vimrc are committed.
