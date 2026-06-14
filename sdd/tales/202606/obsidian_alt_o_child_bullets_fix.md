---
create_time: 2026-06-14 18:46:13
status: wip
prompt: sdd/prompts/202606/obsidian_alt_o_child_bullets_fix.md
---
# Fix: Alt+o / Alt+O Child-Bullet Keymaps Do Not Fire in Obsidian

## Problem

The `<Alt+o>` / `<Alt+O>` child-bullet open-line keymaps added in commit `469a88a` (`task-status-cycler` plugin) do not
work in the live Obsidian vault. The feature was registered through CodeMirror Vim's `vim.mapCommand("<A-o>", ...)` /
`vim.mapCommand("<A-O>", ...)`, but pressing the chords in Vim normal mode does nothing.

## Root Cause

The bindings were registered on the wrong key-handling layer.

In this Obsidian setup, **Alt-modified key events are not delivered to the CodeMirror Vim keymap** — they are consumed
by Obsidian's native hotkey (Scope) layer before CodeMirror Vim's keydown handler can match an `<A-...>` token. As a
result, the two `vim.mapCommand("<A-o>"/"<A-O>")` registrations are dead: the actions are defined and mapped, but the
chords never reach them.

The evidence is decisive and entirely consistent:

1. **Every Alt chord that actually works in this vault is bound through Obsidian commands + `hotkeys.json`, never
   through Vim.** `task-status-cycler:cycle-task-status-forward`/`backward` are bound to `Alt+]` / `Alt+[` in
   `.obsidian/hotkeys.json` and dispatched by `addCommand` (`editorCheckCallback`). The same is true of `Alt+T`
   (duplicate tab), `Alt+P` (templater), and `Alt+Ctrl+Shift+N`. All of these work.

2. **Every working `vim.mapCommand` registration in this plugin uses non-Alt keys** — plain keys (`o`, `O`), `<CR>`,
   `<BS>`, and Ctrl chords (`<C-]>`, `<C-d>`, `<C-u>`, `<C-CR>`). There is _no_ working precedent for an Alt chord
   reaching Vim via `mapCommand` here.

3. **The original plan's justifying premise was factually wrong.** It claimed an existing `<A-y>s` mapping in
   `obsidian_vimrc.md` proved `<A-o>` would work. The vimrc (`/home/bryan/bob/obsidian_vimrc.md`) contains **no Alt
   mappings at all**. The only `<A-y>s` in the system is registered programmatically by the `obsidian-vimrc-support`
   plugin's surround operator (`main.js:1003`), whose own Alt delivery is unverified and not a demonstrated working
   path. So the premise that justified choosing the Vim layer never held.

4. **The original plan explicitly forbade editing `.obsidian/hotkeys.json`** — i.e. it ruled out the one mechanism
   proven to deliver Alt chords in this vault. This is the core mistake this fix corrects.

### Why not "just fix the Vim token"

One alternative hypothesis is that `<A-o>` is merely the wrong token (wrong casing/modifier order). This is rejected as
the likely cause: the surround feature already uses `<A-...>` notation, so the token form is valid; the problem is
delivery of the Alt keydown to the Vim layer, not token spelling. Chasing token variants would be guesswork against a
layer that does not receive the event. The fix below sidesteps the question entirely by moving to the proven Obsidian
hotkey layer.

## Fix Strategy

Re-bind `Alt+o` / `Alt+O` through the **proven Obsidian command + `hotkeys.json` mechanism**, mirroring the existing
`Alt+]` / `Alt+[` task-cycler commands. Crucially, this **reuses the existing, already-validated handler methods
unchanged** — only the trigger mechanism changes.

The bridge that makes this clean: `obsidian-vimrc-support` resolves the CM5 Vim adapter from a `MarkdownView` via
`view.editMode?.editor?.cm?.cm` (its `getCodeMirror(view)`, `main.js:759`), and `task-status-cycler` already exposes the
same shapes (`getEditorViewFromEditor`, `main.js:1762`). An Obsidian `editorCheckCallback(checking, editor, view)`
therefore has direct access to the very `cm` object that `handleVimOpenChildBulletLineBelow(cm)` /
`handleVimOpenChildBulletLineAbove(cm)` already expect — including `enterVimInsertMode(cm)`, which calls
`window.CodeMirrorAdapter.Vim.handleKey(cm, "i", "mapping")` and works because `cm` is the CM5 adapter.

### Changes

**1. `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`**

- **Remove the dead Vim registrations** for the child-bullet actions in `registerVimMappings()`:
  - the two `vim.defineAction("taskStatusCyclerOpenChildBulletLineBelow"/"...Above", ...)` calls
  - the two `vim.mapCommand("<A-o>"/"<A-O>", "action", ...)` blocks

  Keep the handler methods `handleVimOpenChildBulletLineBelow` / `handleVimOpenChildBulletLineAbove` and the
  `getChildBulletOpenLinePrefix` helper (and its export) — they are reused.

- **Add two Obsidian commands** in `onload()`, next to the existing `addCommand` block, e.g.:
  - id `open-child-bullet-line-below`, name "Open child bullet line below"
  - id `open-child-bullet-line-above`, name "Open child bullet line above"

  Each uses `editorCheckCallback(checking, editor, view)` that:
  - resolves the CM5 Vim adapter from the editor (prefer `editor.cm?.cm`, fall back to
    `view?.editMode?.editor?.cm?.cm`);
  - confirms the editor is in **Vim normal mode** (e.g. `cm.state?.vim` present and not `insertMode` / `visualMode` /
    `replaceMode`); if not resolvable or not normal mode, returns `false` so the chord cleanly falls through (this
    preserves the original normal-mode-only intent and avoids inserting in insert mode);
  - when `checking` is `true`, returns whether the above preconditions hold;
  - when `checking` is `false`, calls the corresponding existing handler (`this.handleVimOpenChildBulletLineBelow(cm)` /
    `...Above(cm)`) and returns `true`.

  A small shared private helper (e.g. `resolveNormalModeVimCm(editor, view)`) is acceptable to avoid duplication between
  the two commands.

**2. `/home/bryan/bob/.obsidian/hotkeys.json`**

Add two entries, following this file's existing serialization precedent — note `duplicate-current-tab` records
`Alt`+capital-letter as `{"modifiers": ["Alt"], "key": "T"}`:

```json
"task-status-cycler:open-child-bullet-line-below": [
  { "modifiers": ["Alt"], "key": "o" }
],
"task-status-cycler:open-child-bullet-line-above": [
  { "modifiers": ["Alt"], "key": "O" }
]
```

`Alt+o` (lowercase `o`) → below; `Alt+Shift+o` (capital `O`) → above. No existing hotkey uses `o`/`O`, so there is no
conflict.

### Behavior preserved (unchanged from the approved feature spec)

- `Alt+o` opens a plain child bullet **below**; `Alt+O` opens one **above**.
- Bullet prefix is `currentLeadingWhitespace + "  " + "- "` (one two-space level deeper), always plain `- ` even on task
  lines, current-line indentation never rewritten. Cursor lands after `- `, then enters Vim insert mode. (Identical,
  since the handlers and helper are reused.)
- Existing `o` / `O` continuation mappings and all other Vim/command mappings are untouched.

## Files Expected To Change

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/.obsidian/hotkeys.json`

Both are git-tracked and not ignored, so the fix persists and commits.

## Files Expected NOT To Change

- `/home/bryan/bob/obsidian_vimrc.md`
- plugin manifests / other community plugins
- Markdown note content
- the unrelated pre-existing dirty vault files (`2026/20260614.md`, `bob.md`, `dev.md`, `sase.md`, `sase_blog.md`,
  untracked `ref/chat/...`)
- `bob-cli` Rust/source/docs/tests, memory files

## Implementation Steps

1. Re-check live state immediately before editing:
   - `git -C /home/bryan/bob status --short`
   - `git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json`
2. Edit `main.js`: remove the two child-bullet `defineAction` calls and the two `<A-o>`/`<A-O>` `mapCommand` blocks; add
   the two `addCommand` commands (with the normal-mode CM resolver).
3. Edit `hotkeys.json`: add the two command→chord entries.
4. Static validation:
   - `node --check /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `node -e "JSON.parse(require('fs').readFileSync('/home/bryan/bob/.obsidian/hotkeys.json','utf8'))"`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
5. Focused Node harness (mock `obsidian` + `@codemirror/view`, fake CM5 adapter exposing
   `getCursor/getLine/replaceRange/setCursor/state.vim`):
   - both new commands are registered with the expected ids;
   - check phase returns `false` when `cm.state.vim.insertMode`/`visualMode` is set, `true` in normal mode;
   - execute phase (normal mode) inserts the child-bullet prefix below / above, places the cursor after the prefix, and
     calls insert mode — reusing the existing handler assertions;
   - the child-bullet `<A-o>`/`<A-O>` `mapCommand`/`defineAction` registrations are gone, while `o` / `O` and the other
     Vim mappings remain intact;
   - `getChildBulletOpenLinePrefix` still returns the five documented cases.
6. Manual smoke test (live vault): reload Obsidian / the plugin and the hotkey config, then in Vim normal mode confirm
   `Alt+o` (below) and `Alt+Shift+o` (above) create the child bullet and enter insert mode; confirm they do **not** fire
   in insert mode; confirm `o`/`O`, `Alt+]`/`Alt+[`, and other mappings still work.
7. Review the final vault diff (limited to the two target files), then commit with the SASE git commit workflow, leaving
   the unrelated dirty vault files untouched.

## Validation Summary

- Static: `node --check` on `main.js`, JSON-parse check on `hotkeys.json`, `git diff --check`.
- Harness: command registration + normal-mode gating + below/above insertion & insert-mode + removal of dead Vim
  registrations + prefix cases.
- Manual: live Alt+o / Alt+Shift+o smoke test in normal mode (and confirm no-op in insert mode).

## Risks and Mitigations

- **Risk:** `editor.cm?.cm` shape differs at runtime. **Mitigation:** layered resolution (`editor.cm?.cm` →
  `view?.editMode?.editor?.cm?.cm`), mirroring vimrc-support's proven `getCodeMirror`; bail to `false` if no adapter, so
  nothing breaks.
- **Risk:** Obsidian serializes the `Alt+Shift+o` hotkey differently than `{"modifiers":["Alt"],"key":"O"}`.
  **Mitigation:** the form matches the in-file `duplicate-current-tab` precedent for Alt+capital; the manual smoke test
  confirms registration. If Obsidian re-normalizes it, adjust the single entry to the form Obsidian writes (e.g. add
  `"Shift"`), with no code change.
- **Risk:** the command firing in non-normal modes. **Mitigation:** the check-phase normal-mode gate returns `false`
  outside normal mode, so the chord falls through cleanly.
- **Risk:** OS/window-manager interception of `Alt+o`. **Mitigation:** this is now an Obsidian hotkey (the same layer
  that already delivers `Alt+]`/`Alt+[`/`Alt+T`), so delivery uses the vault's proven path; if a specific chord is still
  intercepted, pick an alternate chord with user approval (config-only change).

## Done Criteria

- `Alt+o` / `Alt+Shift+o` create the child bullet and enter Vim insert mode in the live vault, in normal mode, with the
  unchanged prefix behavior.
- They do not fire in insert mode; `o`/`O` and all other mappings are unchanged.
- The dead `<A-o>`/`<A-O>` Vim registrations are removed.
- Static + harness validation pass; the final vault diff is limited to `main.js` and `hotkeys.json`.
- The change is committed via the SASE git commit workflow with unrelated dirty files untouched.
