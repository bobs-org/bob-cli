---
create_time: 2026-06-11 11:11:45
status: wip
prompt: sdd/prompts/202606/obsidian_ctrl_jk_header_navigation.md
---
# Plan: Obsidian Ctrl+J / Ctrl+K Section-Header Navigation

## Context

Bryan's Obsidian vault is `~/bob` and runs with vim mode enabled (`.obsidian/app.json` has `"vimMode": true`). The local
plugin `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` owns custom navigation behavior (parent/child note
jumps, next/prev link, blank-line insertion, alternate file, etc.). Its commands are wired up two ways:

1. Obsidian editor commands registered in the plugin, bound in `~/bob/.obsidian/hotkeys.json`.
2. CodeMirror Vim normal-mode mappings declared in `~/bob/obsidian_vimrc.md` (loaded by `obsidian-vimrc-support`) using
   the `exmap bob_<name> obcommand bob-navigation-hotkeys:<command-id>` + `nmap <keys> :bob_<name><CR>` pattern
   (precedent: `insert-blank-line-above`/`below`, which are `editorCallback` commands invoked through `obcommand`).

Relevant facts gathered from the vault:

- `hotkeys.json` has no `Ctrl+J` or `Ctrl+K` bindings today, and letter keys there are written uppercase (`"D"`, `"L"`,
  `"P"`, `"R"`).
- **Conflict:** Obsidian's built-in default `Mod+K` (= `Ctrl+K` on Linux) is `editor:insert-link`. The vault does not
  rebind it, so the default is live and must be explicitly cleared for `Ctrl+K` to be available.
- No default or vault binding exists for `Ctrl+J`.
- The plugin already contains the exact frontmatter/fenced-code-block skip machinery needed for header scanning:
  `FRONTMATTER_DELIMITER_RE`, `OPENING_FENCE_RE`/`CLOSING_FENCE_RE`, `startsWithFrontmatter`, `getFenceOpening`,
  `isClosingFence`, and a line-scanning state machine in `findFirstRenderedLink` to mirror.
- The plugin also has reusable editor helpers: `getEditorCursor`, `getEditorLastLine`, `getEditorLineText`, and
  `setEditorCursor` (which sets the cursor _and_ scrolls it into view).
- Pure helpers are exported via `module.exports.helpers` so they can be exercised by ad-hoc Node tests with a mocked
  `obsidian` module (the established validation pattern for this plugin).
- The vault is actively synced by Obsidian Sync and may have pre-existing uncommitted changes; per `~/bob/AGENTS.md`,
  only the files changed for this task may be staged/committed, via `/sase_git_commit`.

## Goal

Add `<Ctrl+J>` and `<Ctrl+K>` keymaps that move the cursor to the **next** and **previous** markdown section header in
the current file, respectively — working both in vim normal mode and while typing in insert mode.

## Behavior Specification

1. Section header definition: an ATX heading line outside YAML frontmatter and outside fenced code blocks, matching
   `^ {0,3}#{1,6}(?:[ \t]|$)` (1–6 `#` with up to 3 leading spaces, followed by whitespace or end of line):
   - `# H1` through `###### H6` count; an empty heading (`##` alone on a line) counts.
   - `#tag` / `#task` lines do NOT count (no whitespace after the hashes), nor does `####### seven` (7 hashes).
   - `# comment` lines inside
     ```/ ~~~ fenced blocks do NOT count (skip using the existing fence state machine, honoring marker char and length like`isClosingFence`
     does).
   - Lines inside the leading `---` frontmatter block do NOT count.
   - Setext headings (`===` / `---` underlines) are deliberately out of scope; the vault uses ATX headings.

2. `<Ctrl+J>` (next): jump to the nearest header line strictly **below** the cursor line. `<Ctrl+K>` (previous): jump to
   the nearest header line strictly **above** the cursor line. A cursor already on a header line therefore jumps to the
   following/preceding header, never to itself.

3. Cursor lands at column 0 of the target header line and is scrolled into view (via the existing `setEditorCursor`
   helper).

4. No target (e.g. `<Ctrl+J>` below the last header, `<Ctrl+K>` above the first, or a file with no headers): the cursor
   does not move and a Notice is shown (`No next section header` / `No previous section header`), matching the plugin's
   existing Notice style.

5. Mode coverage: works in source mode and live preview (editor commands). Reading mode is out of scope (no editor). Vim
   count prefixes (e.g. `3<Ctrl+J>`) are out of scope — the `obcommand` path drops counts, same as the existing
   vimrc-mapped commands.

## Implementation

1. Extend `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (keeps all navigation behavior in the one local
   plugin; no manifest changes needed):
   - Add a `SECTION_HEADER_RE` constant for the ATX pattern above.
   - Add pure top-level helpers (exported via `module.exports.helpers` for testability):
     - `getSectionHeaderLines(lines)` — single top-down pass over the file's lines, skipping the frontmatter block and
       fenced code blocks (same state machine as `findFirstRenderedLink`), returning the array of header line indices.
     - `getSectionHeaderJumpLine(lines, cursorLine, direction)` — picks the first header index strictly greater
       (direction `1`) or the last strictly smaller (direction `-1`) than `cursorLine`; returns `null` when none.
   - Add a plugin method `jumpToSectionHeader(editor, direction)` that reads the cursor (`getEditorCursor`), splits
     `editor.getValue()` on `/\r?\n/`, computes the target line, then either moves the cursor via `setEditorCursor`
     (line, ch 0 — which also scrolls) or shows the appropriate Notice.

2. Register two editor commands in `onload`:
   - `jump-to-next-section-header` / "Jump to next section header" — `editorCallback` →
     `jumpToSectionHeader(editor, 1)`.
   - `jump-to-prev-section-header` / "Jump to previous section header" — `editorCallback` →
     `jumpToSectionHeader(editor, -1)`.

3. Add vim normal-mode mappings to `~/bob/obsidian_vimrc.md`, following the existing exmap/nmap pattern:

   ```
   exmap bob_next_header obcommand bob-navigation-hotkeys:jump-to-next-section-header
   exmap bob_prev_header obcommand bob-navigation-hotkeys:jump-to-prev-section-header
   nmap <C-j> :bob_next_header<CR>
   nmap <C-k> :bob_prev_header<CR>
   ```

   The nmaps guarantee normal-mode behavior even if CodeMirror Vim would otherwise consume the chords before Obsidian's
   hotkey scope sees them; the `hotkeys.json` bindings below cover insert mode and non-vim contexts (an overlapping
   mapping resolves to the same command either way, so there is no double-fire hazard).

4. Update `~/bob/.obsidian/hotkeys.json`:
   - `bob-navigation-hotkeys:jump-to-next-section-header` → `{ "modifiers": ["Ctrl"], "key": "J" }`.
   - `bob-navigation-hotkeys:jump-to-prev-section-header` → `{ "modifiers": ["Ctrl"], "key": "K" }`.
   - `"editor:insert-link": []` — explicitly clears the built-in default `Mod+K` so it cannot conflict with the new
     `Ctrl+K` binding. (Insert-link remains reachable from the command palette; Bryan can rebind it later if wanted.)

## Validation

1. Syntax/config checks:
   - `node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
   - `jq '.' /home/bryan/bob/.obsidian/hotkeys.json`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js .obsidian/hotkeys.json obsidian_vimrc.md`

2. Node helper test with a mocked `obsidian` module covering at least:
   - Simple next/prev jumps across `#`–`######` headings, including from a non-header body line in both directions.
   - Cursor on a header line jumps to the following (next) / preceding (prev) header, not itself.
   - `#tag` text lines and `####### seven-hash` lines are not treated as headers; `   ## indented` (3 spaces) is;
     4-space indented `#` is not; bare `##` at end of line is.
   - `# comment` inside a ``` fence (and a ~~~ fence) is skipped; a header after the closing fence is found.
   - Headers are never found inside the leading `---` frontmatter block; `<Ctrl+J>` from inside frontmatter reaches the
     first real header.
   - Boundary cases return `null` (first header / last header / header-less file), leaving the cursor untouched.

3. Review the final vault diff to confirm only the plugin `main.js`, `hotkeys.json`, and `obsidian_vimrc.md` changed,
   leaving the vault's pre-existing synced changes untouched.

4. Commit only those three vault files via `/sase_git_commit` (required by `~/bob/AGENTS.md`).

## Manual Smoke Test

After reloading Obsidian (or toggling the `bob-navigation-hotkeys` plugin):

1. In a note with several headings, place the cursor in a body paragraph and press `<Ctrl+J>` repeatedly in vim normal
   mode; confirm the cursor walks down through each subsequent heading and shows "No next section header" at the end.
2. Press `<Ctrl+K>` repeatedly; confirm it walks back up and stops at the first heading with the Notice.
3. Enter insert mode and press both chords; confirm the same jumps happen (hotkeys.json path) and that `<Ctrl+K>` no
   longer inserts a markdown link.
4. In a note containing a fenced code block with `# comment` lines and YAML frontmatter, confirm neither is treated as a
   heading target.
