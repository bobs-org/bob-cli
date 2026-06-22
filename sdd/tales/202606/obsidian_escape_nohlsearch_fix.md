---
create_time: 2026-06-22 11:14:14
status: wip
prompt: sdd/prompts/202606/obsidian_escape_nohlsearch_fix.md
---
# Plan: Fix Normal-Mode `<Esc>` Not Clearing Obsidian Vim Search Highlight

## Problem

The previously shipped change added this line to the vault's active Obsidian vimrc (`~/bob/obsidian_vimrc.md`):

```vim
nmap <Esc> :nohlsearch<CR>
```

In live testing this does **nothing**: pressing `<Esc>` in Vim normal mode does not clear the `/` search highlight. We
need to diagnose why and ship a fix that actually works, without regressing insert-mode Escape, visual-mode Escape,
pending-operator Escape, or the existing surround/navigation key handling.

## Root Cause (diagnosed from the code, not guessed)

The mapping is **registered but never triggered**. Two facts establish this:

1. **The mapping is registered.** The Vimrc Support plugin (v0.10.2) feeds every non-comment vimrc line straight to
   CodeMirror Vim via `codeMirrorVimObject.handleEx(cm, line)` (the adapter is `window.CodeMirrorAdapter.Vim`). So
   `nmap <Esc> :nohlsearch<CR>` does create a normal-context keymap entry, and `:nohlsearch` is a real CodeMirror Vim Ex
   command.

2. **Normal-mode `<Esc>` is never dispatched to that mapping.** In Obsidian's CodeMirror 6 + `@replit/codemirror-vim`
   stack, a normal-mode `<Esc>` is handled internally as an input-state reset (it clears any pending operator/count);
   the editor/Vim layer consumes the physical Escape keydown before a user `nmap <Esc>` → Ex mapping is ever looked up
   or executed. The mapping is therefore inert.

This is **corroborated directly inside this codebase** — the maintainer already hit the same class of problem and worked
around it the same way we now must:

- `plugins/bob-navigation-hotkeys/main.js` documents it verbatim for `Ctrl+Shift+J/K`: _"CodeMirror Vim swallows these
  chords before Obsidian's hotkey dispatcher runs ... intentionally avoids a `<C-S-j>`/`<C-S-k>` vim nmap"_ — and
  instead uses a **capture-phase DOM `keydown` listener**.
- `plugins/bob-vim-surround/main.js` intercepts `<Esc>` (to cancel a pending `cs`/`ds`) the same way — a capture-phase
  DOM `keydown` listener checking `event.key === "Escape"`, **not** a vim `nmap`.

Conclusion: a vimrc-only `nmap <Esc>` cannot work for this. The behavior must be implemented at the DOM keydown layer in
a plugin. The prior plan's "fallback" path was, in fact, the only viable path.

## Fix (high-level)

Implement "clear search highlight on normal-mode Escape" in **`bob-navigation-hotkeys`** (the semantically correct
"editor/navigation hotkeys" plugin, and the location the prior approved plan designated for the fallback). The plugin
already contains every primitive needed, so the change is small and idiomatic:

- It already registers **capture-phase `keydown` listeners** on `window`/`document` with proper teardown
  (`registerOpenTaskJumpInputListeners()`).
- It already has `getFocusedMarkdownEditorView(event)` (active `MarkdownView` whose `.cm-editor` contains the event
  target) and `isVimNormalModeEditor(editor, view)` (resolves the CM5 adapter via
  `editor.cm.cm || view.editMode.editor.cm.cm` and returns true only for normal mode).
- Clearing the highlight reuses the exact Ex path the Vimrc Support plugin uses:
  `window.CodeMirrorAdapter.Vim.handleEx(cm, "nohlsearch")`.

### Behavior contract

- **Normal mode, editor focused:** run `nohlsearch` → highlight cleared. A subsequent `/` search still highlights (we
  never disable `hlsearch`; `nohlsearch` only clears the current overlay).
- **Insert / visual / replace mode:** handler returns early — Escape keeps its native meaning (leave insert, leave
  visual, etc.).
- **Not in the editor (modals, command palette, other UI):** handler returns early.
- **No active highlight:** `nohlsearch` is a harmless no-op.
- **Event is NOT consumed.** Unlike the `Ctrl+Shift+J/K` handler (which replaces the key's action and so consumes it),
  this handler only adds a side effect. Leaving the event to propagate means Vim still performs its normal-mode Escape
  (clearing any pending operator/count), and `bob-vim-surround`'s pending-surround Escape cancel is untouched regardless
  of capture-listener ordering. `nohlsearch` is idempotent, so any ordering is safe.

### Sketch (guidance, mirrors existing idioms in the same file)

```js
// onload(): register alongside the existing this.registerOpenTaskJumpInputListeners();
this.registerClearSearchHighlightInputListeners();

registerClearSearchHighlightInputListeners() {
  const keydownHandler = (event) => this.handleClearSearchHighlightKeydown(event);
  const targets = [];
  if (typeof window !== "undefined") targets.push(window);
  if (typeof document !== "undefined" && document !== window) targets.push(document);
  for (const target of targets) {
    if (!target || typeof target.addEventListener !== "function") continue;
    target.addEventListener("keydown", keydownHandler, true);
    this.register(() => target.removeEventListener("keydown", keydownHandler, true));
  }
}

handleClearSearchHighlightKeydown(event) {
  if (!event || (event.key !== "Escape" && event.key !== "Esc")) return false;
  const view = this.getFocusedMarkdownEditorView(event);
  if (!view || !this.isVimNormalModeEditor(view.editor, view)) return false; // insert/visual/etc. fall through
  const cm =
    (view.editor && view.editor.cm && view.editor.cm.cm) ||
    (view.editMode && view.editMode.editor && view.editMode.editor.cm && view.editMode.editor.cm.cm);
  const vim = (typeof window !== "undefined") && window.CodeMirrorAdapter && window.CodeMirrorAdapter.Vim;
  if (!cm || !vim || typeof vim.handleEx !== "function") return false;
  vim.handleEx(cm, "nohlsearch");      // clear current search highlight
  return false;                         // do NOT consume: let Vim still process normal-mode Esc
}
```

(Final implementation should match surrounding code style and may de-dup the `cm` resolution with the existing
`isVimNormalModeEditor` helper.)

## Implementation Steps

1. **Re-check state immediately before editing** (no surprises):
   - `git status --short` in `bob-cli` and in `bob-plugins`;
   - `git -C ~/bob status --short -- obsidian_vimrc.md`.
2. **Edit `plugins/bob-navigation-hotkeys/main.js`** in the `bob-plugins` repo: add the capture-phase Escape listener +
   handler described above, reusing `getFocusedMarkdownEditorView` and `isVimNormalModeEditor`, and wire the
   registration into `onload()` next to `registerOpenTaskJumpInputListeners()`.
3. **Bump `plugins/bob-navigation-hotkeys/manifest.json`** version `1.0.0` → `1.1.0` (new user-facing behavior; the repo
   tracks versions per plugin).
4. **Validate**: run `npm run validate` in `bob-plugins` (manifest/main.js sanity checks).
5. **Remove the inert vimrc mapping** from `~/bob/obsidian_vimrc.md`: delete the `nmap <Esc> :nohlsearch<CR>` line and
   its comment, since the behavior now lives in the plugin and the line is misleading dead config. (Belt-and-suspenders
   is unnecessary and the line genuinely does nothing.)
6. **Deploy** the plugin to the vault with `bob plugins sync` (run with the workspace `-r "<reason>"` / `$PWD` caveat
   noted in long-term memory for SASE workspaces).
7. **Verify in desktop Obsidian** (GUI; requires the user or a GUI-capable run):
   - `/term<CR>` highlights matches → `<Esc>` in normal mode clears them; a fresh `/` still highlights.
   - From insert mode, first `<Esc>` leaves insert mode (unchanged); a second `<Esc>` clears the highlight.
   - Visual mode `<Esc>` still exits visual mode; a pending operator (e.g. `d` then `<Esc>`) still cancels.
   - `cs"'` and `ds"` surround flows, and their `<Esc>` cancels, still work (no regression).
   - Existing nav mappings still work: `-`, `[[`, `]]`, `!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, `\<`, `\>`, `\\`,
     and `Ctrl+Shift+J/K` open-task jumps.
8. **Commit** with the SASE git commit workflow, staging only task-related files:
   - `bob-plugins` repo: `feat(bob-navigation-hotkeys): clear Vim search highlight on normal-mode Escape` (the two
     edited files: `main.js` + `manifest.json`).
   - `~/bob` vault repo: a separate commit removing the dead `nmap <Esc>` line from `obsidian_vimrc.md` (leave the
     vault's other unrelated dirty files untouched).

### Optional diagnostic isolation (only if `nohlsearch` itself is ever in doubt)

To confirm the Ex command clears the highlight independent of the Escape-trigger problem, temporarily add
`nmap \h :nohlsearch<CR>` and press `\h` — it should clear the highlight. This separates "trigger" from "action" but is
not required, since the plugin calls `handleEx(cm, "nohlsearch")` directly and step 7 exercises it end to end. Remove
any such temporary mapping before committing.

## Out of Scope

- No `bob-cli` Rust changes.
- No changes to memory files.
- No changes to Vimrc Support settings, and no enabling of JavaScript vimrc commands.
- No changes to `bob-vim-surround` or the existing `Ctrl+Shift+J/K` handler — only an additive sibling handler in
  `bob-navigation-hotkeys`.
- Not attempting any further vimrc-level `nmap <Esc>` variant — established as structurally non-functional.

## Why This Is The Right Fix

- It targets the actual root cause (DOM-level Escape interception) instead of re-attempting a mapping the stack will
  keep ignoring.
- It reuses proven, in-repo machinery (`bob-navigation-hotkeys`' own capture-phase listener, focus/mode helpers, and the
  Vimrc Support `handleEx` Ex path), so it stays consistent with how the codebase already solves "Vim swallows the key"
  problems.
- The non-consuming, normal-mode-only design keeps every other Escape behavior (insert/visual exit, pending-operator
  cancel, surround cancel) exactly as it is today.
