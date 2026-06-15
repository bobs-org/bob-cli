---
title: 'Fix #2: Alt+o / Alt+O Child-Bullet Keymaps Still Dead — Broken Normal-Mode
  Gate'
create_time: 2026-06-15 07:15:05
status: proposed
prompt: sdd/prompts/202606/obsidian_alt_o_child_bullets_fix2.md
---

# Fix #2: Alt+o / Alt+O Child-Bullet Keymaps Still Dead — Broken Normal-Mode Gate

## Problem

The `Alt+o` / `Alt+O` child-bullet open-line keymaps still do nothing in the live Obsidian vault, even after the first
fix (commit `1816993`) re-routed them from the CodeMirror Vim layer to the Obsidian command + `hotkeys.json` layer.

Current live state (all confirmed on disk):

- `.obsidian/hotkeys.json` has `task-status-cycler:open-child-bullet-line-below` → `Alt+o` and `...-above` → `Alt+O`,
  following the same serialization as the working `Alt+]` / `Alt+[` entries.
- `task-status-cycler/main.js` registers both commands via `addCommand({ id, name, editorCheckCallback })`, reusing the
  validated handlers `handleVimOpenChildBulletLineBelow/Above(cm)` and the `getChildBulletOpenLinePrefix` helper.
- The plugin is enabled; the working tree matches the committed fix.

So the first fix chose the **right mechanism** but the chords are still inert. This plan finds out exactly why and fixes
the real defect.

## Root-Cause Analysis (differential)

The decisive evidence is a side-by-side comparison of a working Alt command and the broken one — both live in the same
plugin and are registered identically:

|                                      | `Alt+]` (works)                                     | `Alt+o` (dead)                                            |
| ------------------------------------ | --------------------------------------------------- | --------------------------------------------------------- |
| Registration                         | `addCommand` + `hotkeys.json`                       | `addCommand` + `hotkeys.json` (identical)                 |
| Callback type                        | `editorCheckCallback(checking, editor, view)`       | `editorCheckCallback(checking, editor, view)` (identical) |
| `view instanceof MarkdownView` guard | yes — passes                                        | yes — same guard                                          |
| What the **check** depends on        | `getActiveTaskStatus(editor)` (Obsidian editor API) | `resolveNormalModeVimCm(editor, view)`                    |

Because `Alt+]` works, this vault **does** deliver `Alt`-modified chords to Obsidian's editor-command/hotkey dispatch
while CodeMirror/Vim is focused, and `editorCheckCallback` commands **do** fire on those chords. `Alt+T` (duplicate tab)
and `Alt+P` (templater) corroborate this. So event delivery is **not** the differentiator.

The only material difference is the check callback. `Alt+]`'s check is robust (Obsidian editor API). `Alt+o`'s check is
gated by `resolveNormalModeVimCm`, and an Obsidian `editorCheckCallback` that returns `false` during the _checking_
phase makes the command inapplicable, so the bound hotkey does nothing. **Therefore the defect is isolated to
`resolveNormalModeVimCm` returning `null`.**

### Why `resolveNormalModeVimCm` is the wrong gate

```js
resolveNormalModeVimCm(editor, view) {
  const editorCm = editor && editor.cm && editor.cm.cm;
  const viewCm = view && view.editMode && view.editMode.editor
    && view.editMode.editor.cm && view.editMode.editor.cm.cm;
  const cm = editorCm || viewCm;
  const vimState = cm && cm.state && cm.state.vim;
  if (!vimState || vimState.insertMode || vimState.visualMode || vimState.replaceMode) {
    return null;            // <-- returns null whenever vimState is falsy → command dead
  }
  return cm;
}
```

This code reads vim mode as **CM5 boolean flags** (`insertMode` / `visualMode` / `replaceMode`). That shape is never
used or verified anywhere else in this vault. The proven-working references read the _same_ `editor.cm.cm` object's mode
differently:

- `mrj-jump-to-link/main.js:843,858` resolves `cm = editor.cm.cm` (identical to here) and reads its mode as
  `cm.state.vim?.mode` — a **string** (`'normal' | 'insert' | 'visual' | 'replace'`), not boolean flags.
- `obsidian-vimrc-support` never reads those booleans; it tracks mode from the CM5 `vim-mode-change` event
  (`main.js:561`, `this.isInsertMode = mode === 'insert'`).

Two failure modes follow from the boolean-flag gate, and either one produces the exact "dead chord" symptom — this is a
**fail-closed default**:

1. If `cm.state.vim` is falsy at check time (e.g. the resolved adapter does not expose vim state where this code looks,
   or it is lazily attached), the `!vimState` branch returns `null` → check returns `false` → chord inert. This is the
   most likely live cause and is consistent with `mrj`'s defensive `?.` on the same access.
2. Even if `cm.state.vim` is truthy with only a `.mode` string, the boolean guards are all `undefined`, so the
   insert-mode protection silently never fires — a latent correctness bug.

The first fix introduced this gate as brand-new, **never-run-in-Obsidian** code; it is the only new logic between the
working `o`/`O` handlers and the dead Alt commands. That matches the pattern of both prior attempts failing on
unverified assumptions about the Obsidian/CodeMirror runtime.

## Diagnosis-First: confirm the exact trigger before editing

The previous two attempts each shipped a plausible static hypothesis that turned out wrong in the live runtime. This
plan **breaks that cycle by requiring one runtime observation before any code change**, because the fix must know
whether `cm` fails to resolve or `cm.state.vim` is simply the wrong shape.

In the live GUI Obsidian, with an open note in **Vim normal mode**, run in the developer console (DevTools):

```js
const v = app.workspace.getActiveViewOfType(this.MarkdownView ?? require("obsidian").MarkdownView);
const ed = v.editor;
const cm = ed?.cm?.cm || v?.editMode?.editor?.cm?.cm;
console.log("cm?", !!cm, "state.vim?", !!(cm && cm.state && cm.state.vim), cm && cm.state && cm.state.vim);
```

Plus verify the command is actually reachable: open the command palette and confirm "Open child bullet line below"
exists and, when invoked from the palette in normal mode, inserts the child bullet (this isolates the _hotkey/check_
path from the _handler_ path).

Expected outcomes and what each proves:

- `cm` falsy → resolution path is wrong; the fix must broaden cm resolution (Failure Mode 1, resolution variant).
- `cm` truthy but `state.vim` falsy → fail-closed via `!vimState` (Failure Mode 1, state variant) — the dominant
  hypothesis.
- `cm.state.vim` truthy with a `.mode` string and no booleans → the gate passes in all modes, so the chord should
  already fire; if it still does not, the problem is delivery-specific to `o`/`O` and we re-open the event-layer
  question. (Considered unlikely given `Alt+]` works, but explicitly checked rather than assumed.)
- Palette invocation works but the hotkey does not → confirms the defect is the **check gate**, not the handler.

The fix below is robust to all of cases 1–2 (it does not depend on which one is live), so implementation can proceed in
the same session once the console confirms it is the check gate (which the palette test alone establishes).

## Fix Strategy

Rewrite `resolveNormalModeVimCm` to be **robust and fail-open**, and to detect non-normal modes across both the CM5
boolean shape and the Obsidian/CM6 `.mode` string shape. Keep everything else (the two `addCommand` registrations, the
two handlers, the prefix helper, and both `hotkeys.json` entries) unchanged — they are correct.

Design principles:

1. **Resolve `cm` robustly.** Keep the proven `editor.cm.cm` / `view.editMode.editor.cm.cm` paths (used verbatim by
   `vimrc-support` and `mrj-jump-to-link`), and only treat the result as usable if it exposes the editing methods the
   handlers need (`getCursor`, `getLine`, `replaceRange`, `setCursor`). If the console step shows resolution itself
   fails, add the active-`MarkdownView` fallback that the existing Vim handlers already rely on.

2. **Fail open, not closed.** The command should fire whenever a usable Vim `cm` exists. Only _positively detected_
   insert / visual / replace mode should suppress it. Absence/unknown vim state must **not** kill the command — that
   default-deny is the current bug.

3. **Detect mode across both shapes.** Treat the editor as non-normal when either the CM5 boolean is set
   (`insertMode === true`, etc.) **or** the string mode matches
   (`mode === 'insert' | 'visual' | 'visual-block' | 'visual-line' | 'replace'`). This makes the intended
   normal-mode-only behavior actually work (fixing the latent bug from Failure Mode 2) while no longer being the thing
   that silently disables the feature.

Resulting shape (illustrative, not prescriptive):

```js
resolveNormalModeVimCm(editor, view) {
  const cm = (editor && editor.cm && editor.cm.cm)
    || (view && view.editMode && view.editMode.editor
        && view.editMode.editor.cm && view.editMode.editor.cm.cm)
    || null;
  if (!cm
      || typeof cm.getCursor !== "function"
      || typeof cm.getLine !== "function"
      || typeof cm.replaceRange !== "function"
      || typeof cm.setCursor !== "function") {
    return null; // no usable Vim editor → let the chord fall through
  }
  const vimState = cm.state && cm.state.vim;
  if (vimState) {
    const mode = typeof vimState.mode === "string" ? vimState.mode : null;
    const inInsert  = vimState.insertMode  === true || mode === "insert";
    const inVisual  = vimState.visualMode  === true || mode === "visual"
                       || mode === "visual-block" || mode === "visual-line";
    const inReplace = vimState.replaceMode === true || mode === "replace";
    if (inInsert || inVisual || inReplace) {
      return null; // positively non-normal → fall through cleanly
    }
  }
  return cm; // normal mode, or vim state not introspectable → fire (fail open)
}
```

### Behavior preserved (unchanged from the approved feature spec)

- `Alt+o` opens a plain child bullet **below**; `Alt+O` opens one **above**.
- Prefix is `currentLeadingWhitespace + "  " + "- "`, always plain `- ` even on task lines; current-line indentation is
  never rewritten; cursor lands after `- `; then Vim insert mode is entered. (Handlers and helper are reused unchanged.)
- Existing `o` / `O` continuation and all other Vim/command mappings are untouched.
- The command remains normal-mode-intended; in confidently-detected insert/visual/replace mode it falls through.

## Files Expected To Change

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js` — rewrite `resolveNormalModeVimCm` only (plus, if the
  console step requires it, broaden the cm resolution fallback). No change to handlers, helper, `addCommand` blocks, or
  the dead-Vim-mapping removal already done.

## Files Expected NOT To Change

- `/home/bryan/bob/.obsidian/hotkeys.json` — the two entries are correct (they match the working `Alt+]`/`Alt+T`
  precedent); no edit needed unless the console step surprises us.
- `/home/bryan/bob/obsidian_vimrc.md`, other plugins/manifests, Markdown note content.
- The unrelated pre-existing dirty vault files (`bob.md`, `gtd_daily.md`, `mac_inbox.md`, `sase.md`, `sase_blog.md`,
  untracked `2026/20260615.md`).
- `bob-cli` Rust/source/docs/tests, memory files.

## Implementation Steps

1. Re-check live state immediately before editing:
   - `git -C /home/bryan/bob status --short`
   - `git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js .obsidian/hotkeys.json`
2. **Runtime confirmation (DevTools)** per the "Diagnosis-First" section: log `cm` / `cm.state.vim`, and confirm the
   command fires from the command palette in normal mode. Record which failure mode is live. Do not proceed to a code
   edit until the check gate is confirmed as the defect (the palette test alone suffices).
3. Edit `main.js`: replace `resolveNormalModeVimCm` with the robust, fail-open, dual-shape version. If — and only if —
   step 2 showed `cm` itself fails to resolve, add the active-`MarkdownView` fallback used by the existing Vim handlers.
4. Static validation:
   - `node --check /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
5. Focused Node harness (mock `obsidian`; fake CM exposing `getCursor/getLine/replaceRange/setCursor` and a settable
   `state.vim`):
   - normal mode (no `state.vim`, or `state.vim` with neither booleans nor a non-normal `.mode`) → resolver returns the
     cm; `editorCheckCallback` checking-phase returns `true`.
   - `state.vim.mode === 'insert'` **and** `state.vim.insertMode === true` → resolver returns `null` (both shapes).
   - `state.vim.mode === 'visual'` / `'replace'` → resolver returns `null`.
   - cm missing required methods → resolver returns `null`.
   - execute phase in normal mode inserts the child-bullet prefix below/above, cursor after the prefix, enters insert
     mode (reusing the existing handler assertions); `getChildBulletOpenLinePrefix` still returns its five documented
     cases.
6. Manual smoke test (live GUI Obsidian): reload the plugin/hotkeys, then in Vim **normal** mode confirm `Alt+o` (below)
   and `Alt+Shift+o` (above) create the child bullet and enter insert mode; confirm they fall through in **insert**
   mode; confirm `o`/`O`, `Alt+]`/`Alt+[`, `Alt+T` still work.
7. Review the final vault diff (limited to `main.js`), then commit with the SASE git commit workflow, leaving the
   unrelated dirty vault files untouched.

## Validation Summary

- Static: `node --check` on `main.js`; `git diff --check`.
- Harness: resolver returns cm in normal/unknown state; returns null for both boolean and `.mode`-string non-normal
  states and for an unusable cm; below/above insertion + insert-mode; prefix cases intact.
- Manual: live `Alt+o` / `Alt+Shift+o` fire in normal mode, fall through in insert mode; siblings unaffected.

## Risks and Mitigations

- **Risk:** the live failure is actually cm-resolution, not the state gate. **Mitigation:** the mandatory DevTools step
  distinguishes these before editing; the fix adds the active-view fallback only if needed.
- **Risk:** fail-open lets `Alt+o` insert a bullet if pressed in insert mode when vim state is not introspectable.
  **Mitigation:** dual-shape detection blocks every mode shape observed in this vault; a non-introspectable state is
  rare, and a stray child bullet is strictly preferable to a permanently dead keymap. Revisit only if the smoke test
  shows a real insert-mode misfire.
- **Risk:** a third unseen runtime detail (the pattern that broke attempts 1 and 2). **Mitigation:** this plan does not
  ship a new static guess — it requires a live observation that the chord reaches the command and that the check gate is
  the failing link before committing a code change.

## Done Criteria

- `Alt+o` / `Alt+Shift+o` create the child bullet and enter Vim insert mode in the live vault, in normal mode, with the
  unchanged prefix behavior; they fall through in insert mode.
- `o`/`O`, `Alt+]`/`Alt+[`, and all other mappings are unchanged.
- The DevTools observation that identified the live failure mode is recorded in the implementation notes/commit.
- Static + harness validation pass; the final vault diff is limited to `main.js` (and only `main.js`).
- The change is committed via the SASE git commit workflow with unrelated dirty files untouched.
