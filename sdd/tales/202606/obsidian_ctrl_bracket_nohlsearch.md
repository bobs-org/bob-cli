---
create_time: 2026-06-22 11:26:54
status: wip
prompt: sdd/prompts/202606/obsidian_ctrl_bracket_nohlsearch.md
---
# Plan: Make `<C-[>` Also Clear Obsidian Vim Search Highlight (Escape parity)

## Problem

The just-shipped `bob-navigation-hotkeys` change clears the Vim `/` search highlight when the user presses `<Esc>` in
normal mode. In live testing it works for the physical **Escape** key but does **nothing** for **`<C-[>`** (Ctrl+`[`),
which Vim users expect to be a perfect synonym for `<Esc>`.

We need `<C-[>` in normal mode to clear the search highlight exactly like `<Esc>` already does, without regressing the
existing `<Esc>` behavior, insert/visual/replace-mode handling, the `bob-vim-surround` Escape cancel, or the
`Ctrl+Shift+J/K` open-task-jump handler.

## Root Cause (diagnosed from the code, not guessed)

The capture-phase handler added in the prior fix gates entirely on the _DOM key name_:

```js
// plugins/bob-navigation-hotkeys/main.js — handleClearSearchHighlightKeydown()
if (!event || (event.key !== "Escape" && event.key !== "Esc")) {
  return false;
}
```

`<C-[>` never satisfies this predicate. In Obsidian's Electron/Chromium DOM, pressing Ctrl+`[` dispatches a `keydown`
with:

- `event.key === "["` (the logical character — **not** `"Escape"`),
- `event.code === "BracketLeft"`,
- `event.ctrlKey === true`.

The DOM does **not** fold `<C-[>` into an Escape key event. The only layer that treats `<C-[>` as Escape is **CodeMirror
Vim itself**, and it does so _internally_ — after our capture-phase `keydown` listener has already run and returned
early. So for `<C-[>` the handler bails before ever calling `vim.handleEx(cm, "nohlsearch")`, and the highlight stays.
(The physical `<Esc>` key works only because it _does_ arrive as `event.key === "Escape"`.)

This is the same class of "Vim/CodeMirror translates the key after the DOM" issue the codebase already documents, and
the fix is symmetric with how the existing handler already detects the `Ctrl+Shift+J/K` chord by inspecting `event.code`
/ `event.ctrlKey` (`getOpenTaskJumpKeydownDirection()`), rather than relying on a single `event.key` name.

## Fix (high-level)

Broaden the handler's escape-detection so it recognizes **both** the physical Escape key **and** the `<C-[>` chord, then
reuse the existing normal-mode + `nohlsearch` machinery unchanged.

Introduce a small predicate (mirroring the existing `getOpenTaskJumpKeydownDirection()` chord-matching idiom in the same
file) and call it from `handleClearSearchHighlightKeydown()` in place of the current inline `event.key` check:

```js
isClearSearchHighlightEscapeKeydown(event) {
  if (!event) return false;
  // Physical Escape key (already working today).
  if (event.key === "Escape" || event.key === "Esc") return true;
  // Vim treats Ctrl+[ as a synonym for <Esc>, but the Electron/Chromium DOM does
  // NOT translate the chord into an Escape key event (CodeMirror Vim does that
  // internally, after this capture-phase listener has already run). Match the
  // chord explicitly so normal-mode Ctrl+[ clears the search highlight too.
  return (
    event.ctrlKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.shiftKey &&
    (event.code === "BracketLeft" || event.key === "[")
  );
}
```

Then in `handleClearSearchHighlightKeydown()` replace:

```js
if (!event || (event.key !== "Escape" && event.key !== "Esc")) {
  return false;
}
```

with:

```js
if (!this.isClearSearchHighlightEscapeKeydown(event)) {
  return false;
}
```

Everything downstream is untouched: the WeakSet de-dup, `getFocusedMarkdownEditorView()`, `isVimNormalModeEditor()`,
`resolveVimCodeMirror()`, and the `vim.handleEx(cm, "nohlsearch")` call all stay exactly as they are. The handler still
**returns `false` and never calls `preventDefault()`/`stopPropagation()`**, so `<C-[>` keeps propagating and CodeMirror
Vim still performs its own normal-mode Escape semantics — we only add the idempotent `nohlsearch` side effect.

### Behavior contract (after the change)

- **Normal mode, editor focused, `<Esc>`:** clears highlight (unchanged).
- **Normal mode, editor focused, `<C-[>`:** clears highlight (the fix).
- **Insert / visual / replace mode (`<Esc>` or `<C-[>`):** handler returns early — the key keeps its native meaning
  (leave insert/visual, etc.). As before, from insert mode the first press leaves insert mode and a second press (now
  either `<Esc>` or `<C-[>`) clears the highlight.
- **Not in the editor (modals, command palette, other UI):** returns early.
- **No active highlight:** `nohlsearch` is a harmless no-op.
- **Chord precision:** only Ctrl+`[` with no Alt/Meta/Shift matches. `Ctrl+Shift+[` (`{`), `Ctrl+Alt+[`, and the
  `Ctrl+Shift+J/K` jump chords are all excluded, so no other handler is affected.

## Scope of detection (deliberately limited)

- Handle **only** `<Esc>` and `<C-[>`. We do **not** add `<C-c>`: in Vim `<C-c>` is a near-but-not-equal Escape synonym
  (skips `InsertLeave` autocmds, different pending-state semantics) and it collides with copy; matching the user's
  request and avoiding surprises, `<C-c>` stays out of scope.

## Implementation Steps

1. **Re-check state immediately before editing:**
   - `git status --short` in the `bob-cli` workspace and in the `bob-plugins` linked repo;
   - `git -C ~/bob status --short -- obsidian_vimrc.md` (should be clean / already committed from the prior fix).
2. **Edit `plugins/bob-navigation-hotkeys/main.js`** in the `bob-plugins` linked repo:
   - Add the `isClearSearchHighlightEscapeKeydown(event)` predicate next to the other keydown helpers (e.g. near
     `getOpenTaskJumpKeydownDirection()`), matching surrounding code style and comment density.
   - Replace the inline `event.key` check at the top of `handleClearSearchHighlightKeydown()` with a single call to the
     new predicate.
3. **Bump `plugins/bob-navigation-hotkeys/manifest.json`** version `1.1.0` → `1.1.1` (bug fix to the behavior shipped in
   `1.1.0`; the repo tracks versions per plugin).
4. **Validate (non-GUI):**
   - `npm run validate` in `bob-plugins` (manifest/main.js sanity checks);
   - `node --check plugins/bob-navigation-hotkeys/main.js` (parse check on the large file).
5. **Deploy** the plugin to the vault with `bob plugins sync` for `bob-navigation-hotkeys`, using the workspace
   `-r "<reason>"` / `$PWD` caveat recorded in long-term memory for SASE workspaces.
6. **Verify in desktop Obsidian** (GUI; requires the user or a GUI-capable run):
   - `/term<CR>` highlights matches → in **normal mode**, `<C-[>` clears them (the fix); a fresh `/` still highlights.
   - `<Esc>` in normal mode still clears them (no regression).
   - From insert mode: first `<C-[>` leaves insert mode (unchanged), a second `<C-[>` clears the highlight; same for
     `<Esc>`.
   - Visual mode `<C-[>` / `<Esc>` still exit visual mode; a pending operator (`d` then `<C-[>`) still cancels.
   - `cs"'` / `ds"` surround flows and their Escape cancel still work (no regression).
   - Existing nav mappings still work: `-`, `[[`, `]]`, `!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, `\<`, `\>`, `\\`,
     and `Ctrl+Shift+J/K` open-task jumps (confirm `<C-[>` matching did not disturb them).
7. **Commit** with the SASE git commit workflow, staging only task-related files:
   - `bob-plugins` repo: `fix(bob-navigation-hotkeys): clear Vim search highlight on Ctrl+[ as well as Escape` (the two
     edited files: `main.js` + `manifest.json`).

## Out of Scope

- No `bob-cli` Rust changes.
- No changes to memory files.
- No vault `obsidian_vimrc.md` change (the inert `nmap <Esc>` line was already removed in the prior fix; nothing new is
  needed there).
- No changes to `bob-vim-surround`, the `Ctrl+Shift+J/K` handler, or any consumption/`preventDefault` behavior — this is
  a purely additive broadening of the existing escape predicate.
- No `<C-c>` or other Escape-synonym handling beyond `<Esc>` and `<C-[>`.
- No vimrc-level mapping attempt (established as structurally non-functional in the prior fix).

## Why This Is The Right Fix

- It targets the real root cause: the handler keyed on `event.key === "Escape"`, and `<C-[>` is delivered to the DOM as
  `BracketLeft` + `ctrlKey`, never as an Escape key event.
- It mirrors an idiom already in the same file (`getOpenTaskJumpKeydownDirection()` matches chords via
  `event.code`/`event.ctrlKey`), keeping the codebase consistent.
- It is minimal and non-consuming: every other Escape/`<C-[>` behavior (insert/visual exit, pending-operator cancel,
  surround cancel, nav chords) is preserved because the event still propagates to CodeMirror Vim untouched.
