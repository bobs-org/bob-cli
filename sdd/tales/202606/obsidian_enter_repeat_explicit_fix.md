---
create_time: 2026-06-06 07:36:38
status: done
prompt: sdd/prompts/202606/obsidian_enter_repeat_explicit_fix.md
---
# Obsidian Vim Enter — `repeatIsExplicit` Root-Cause Fix

## Goal

Make Vim normal-mode bare `<Enter>` in the Bob Obsidian vault target the **current** cursor line (not the line below it)
for link open/create. Preserve counted Enter (`5<Enter>` → 5 lines down), no-link fallthrough movement, Backspace
previous-line link behavior, and Ctrl+Enter task toggling.

## Root Cause (newly diagnosed — prior agents misdiagnosed this)

The bug is **not** a stale plugin runtime (the conclusion of the prior `obsidian_enter_current_line` tale, status
`done`). The source is genuinely wrong, but in a way the prior verification could not detect because its test stubs did
not model CodeMirror's real action arguments.

### The actual data flow

1. `.obsidian.vimrc` does **not** map `<CR>`. The mapping lives in `task-status-cycler/main.js`:
   `vim.mapCommand("<CR>", "action", "taskStatusCyclerOpenNextLineLink", …)` → `handleVimEnterLinkOrFallthrough` →
   delegates to `bob-navigation-hotkeys.handleVimEnterLinkAction(cm, actionArgs)` →
   `handleVimLineLinkAction(cm, actionArgs, /*direction*/ 1, /*defaultOffset*/ 0)` → `getVimOffsetTargetLine` →
   `getVimTargetOffset` → **`hasVimRepeat(actionArgs)`**.

2. `getVimTargetOffset` returns the per-key `defaultOffset` (0 for Enter → current line) **only when
   `hasVimRepeat(actionArgs)` is false**. Otherwise it returns `direction * getVimRepeat(actionArgs)`.

3. `hasVimRepeat` (in `bob-navigation-hotkeys/main.js`, lines 27-33) decides "was a count typed?" purely by:
   ```js
   actionArgs.repeat !== undefined && actionArgs.repeat !== null;
   ```

### Why that check is wrong

Obsidian's bundled CodeMirror vim, in its action-dispatch path, sets the arguments like this (verified by extracting the
strings directly from `/snap/obsidian/62/resources/obsidian.asar`):

```js
var repeat = inputState.getRepeat();          // 0 when the user typed no count
var repeatIsExplicit = !!repeat;              // → false for a bare <CR>
...
actionArgs.repeat = repeat || 1;              // → ALWAYS a number; defaults to 1
actionArgs.repeatIsExplicit = repeatIsExplicit; // → false for a bare <CR>
```

So for a bare `<Enter>` the real `actionArgs` is `{ repeat: 1, repeatIsExplicit: false }` — `repeat` is **never**
`undefined`/`null`. Therefore `hasVimRepeat` returns `true`, `getVimTargetOffset` returns `1 * 1 = 1`, and Enter targets
`cursor.line + 1` — the line **after** the cursor. That is precisely the reported bug.

`5<Enter>` happens to work only by coincidence: `{ repeat: 5, repeatIsExplicit: true }` → offset `5` →
`cursor.line + 5`.

### Why every prior attempt "passed" verification yet stayed broken

The prior plans verified behavior with a Node VM check whose stub `actionArgs` were `{}`, `{ repeat: undefined }`, and
`{ repeat: 1 }`. None of those carry `repeatIsExplicit`, and crucially none reproduce CodeMirror's real bare-Enter
payload (`{ repeat: 1, repeatIsExplicit: false }`). The stub `{ repeat: 1 }` was treated as "an explicit count of 1," so
the test reported `explicitRepeat1 = 11` and concluded the code was correct. The verification diverged from reality,
producing a false "already fixed; just reload Obsidian" conclusion. The real signal CodeMirror uses to mean "no count
was typed" — `repeatIsExplicit === false` — was never part of the logic or the tests.

## The Fix

Single-function change in `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`: make `hasVimRepeat`
consult CodeMirror's `repeatIsExplicit` flag, which is the authoritative "was a count typed?" signal.

```js
function hasVimRepeat(actionArgs) {
  if (!actionArgs) {
    return false;
  }
  // CodeMirror's Vim always sets actionArgs.repeat (defaulting to 1 when no
  // count is typed) and signals an explicitly-typed count via repeatIsExplicit.
  // Trust that flag when present; bare <Enter> arrives as
  // { repeat: 1, repeatIsExplicit: false } and must be treated as "no count".
  if (typeof actionArgs.repeatIsExplicit === "boolean") {
    return actionArgs.repeatIsExplicit;
  }
  // Fallback for callers/tests that omit repeatIsExplicit.
  return actionArgs.repeat !== undefined && actionArgs.repeat !== null;
}
```

### Why this is correct and safe

- **Bare `<Enter>`** (`{ repeat: 1, repeatIsExplicit: false }`): `hasVimRepeat` → `false` → offset `0` → current line.
  Fixed.
- **`5<Enter>`** (`{ repeat: 5, repeatIsExplicit: true }`): `hasVimRepeat` → `true` → offset `5` → 5 lines down.
  Unchanged.
- **`1<Enter>`** (`{ repeat: 1, repeatIsExplicit: true }`): `hasVimRepeat` → `true` → offset `1` → one line down. Now
  genuinely distinguishable from a bare Enter (the old code could not tell these apart).
- **Bare Backspace** (`{ repeat: 1, repeatIsExplicit: false }`): `hasVimRepeat` → `false` → `defaultOffset = -1` →
  previous line. Identical to before (old code also yielded `-1`), so no regression.
- **Fallthrough movement** (no actionable link) is computed separately via `getVimRepeat`, which still normalizes to
  `1`; bare Enter with no link still moves down one line. Unchanged.
- **Backward-compatible** with the prior VM tests: `{}` → `false` (current line), `{ repeat: 1 }` → `true` via the
  fallback (those tests still pass), while the live CodeMirror payload is now handled correctly via the flag.

No change is needed in `task-status-cycler/main.js`: it delegates link targeting to the navigation plugin and only uses
`getVimRepeat` for fallthrough movement (correctly defaulting to 1). `.obsidian.vimrc`, `.obsidian/hotkeys.json`, and
task-completion behavior are untouched.

## Scope

- **Edit:** `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` — only the `hasVimRepeat` function.
- **No edits:** `task-status-cycler/main.js`, `.obsidian.vimrc`, `.obsidian/hotkeys.json`, any Rust/`bob-cli` source, or
  any of the unrelated dirty vault files (`community-plugins.json`, `_templates/new_note.md`, `bob.md`, `gtd_daily.md`,
  `ref/chat/…`, `sase.md`, `2026/20260606_day.md`).

## Acceptance Criteria

- Bare normal-mode `<Enter>` reads links from the cursor's **current** line.
- Bare `<Enter>` opens/creates a single actionable current-line link; opens the picker for multiple.
- Bare `<Enter>` with no actionable current-line link still falls through to one-line-down movement.
- `1<Enter>` targets one line below the cursor; `5<Enter>` targets five lines below — for link open/create.
- Bare Backspace link behavior still targets the previous line.
- Ctrl+Enter task toggle behavior is unchanged.
- Unrelated dirty vault files remain untouched.

## Verification Plan

### Static checks

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

### Realistic VM check — this is the key improvement over prior attempts

Stub `actionArgs` must match CodeMirror's real payloads, including `repeatIsExplicit`. Assert:

- `getVimEnterTargetLine(cm, { repeat: 1, repeatIsExplicit: false })` → `cursor.line` (the live bare-Enter case).
- `getVimEnterTargetLine(cm, { repeat: 5, repeatIsExplicit: true })` → `cursor.line + 5`.
- `getVimEnterTargetLine(cm, { repeat: 1, repeatIsExplicit: true })` → `cursor.line + 1`.
- `getVimBackspaceTargetLine(cm, { repeat: 1, repeatIsExplicit: false })` → `cursor.line - 1`.
- Backward-compat: `getVimEnterTargetLine(cm, {})` → `cursor.line`; `getVimEnterTargetLine(cm, { repeat: 1 })` →
  `cursor.line + 1` (fallback path, prior tests still green).

### Live confirmation (recommended after reload)

In Obsidian, reload (or toggle `bob-navigation-hotkeys`) so the patched code loads, then:

- Cursor on a line with one `[[note]]`; press `<Enter>` → that note opens (no longer the line below).
- Cursor on a line with multiple links; `<Enter>` → picker opens for the current line.
- `5<Enter>` where the line five below has a link → opens that target.

## Commit Plan

After implementing and verifying, commit only the single changed file
(`.obsidian/plugins/bob-navigation-hotkeys/main.js`) under `~/bob` using `/sase_git_commit`. Do not stage, revert, or
commit any unrelated dirty vault files. Also mark this SDD plan/tale done per the normal SDD workflow.
