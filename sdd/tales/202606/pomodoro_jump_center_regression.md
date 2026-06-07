---
create_time: 2026-06-07 08:34:52
status: done
prompt: sdd/prompts/202606/pomodoro_jump_center_regression.md
---
# Plan: Fix Broken `zz`-Style Recenter for the `\\` Obsidian Pomodoro Jump

## Goal

Restore the Vim `zz`-style "redraw from the middle" behavior for the Bob Ledger Tools `\\` keymap. After pressing `\\`
in a note that has a local Pomodoro target, the cursor must land on the target line **and the viewport must center on
that line** (like Neovim's `zz`). This regressed recently; the cursor still moves, but the line is no longer centered.

Expected behavior (all must hold):

- `\\` with a **local** `## Pomodoros` target: jump to the target line in the current editor and **center** it.
- `\\` with **no local target**, daily note already open: activate the daily tab, jump to its Pomodoro line, and
  **center** it (the behavior the recent daily work was trying to add — must not regress).
- `\\` with **no local target**, daily note not open: keep the Daily Notes command / open-file fallback, then center.
- `\\` on today's already-active daily note with no target: keep the existing no-target notice, no reopen loop.

## Context Reviewed

- Plugin under change (the only file to edit): `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- `~/bob/AGENTS.md`: the vault is actively synced; inspect `git status` before editing; do **not** stage/revert/commit
  unrelated pre-existing changes; commit only the task file via `/sase_git_commit` before terminating.
- This change touches no CLI subcommands or options, so the `memory/long/cli_rules.md` tier-2 read does not apply.
- Related history in the plugin repo (`/home/bryan/bob`):
  - `5e60f6a feat: center pomodoro jump in editor` — introduced centering.
  - `cbe91fa fix: center pomodoro jump past vim trailing scroll` — **last known-good** centering: deferred one frame so
    our centered scroll wins over codemirror-vim's trailing "nearest" scroll.
  - `3dba1e0`, `e568a7d`, `d793951` — added the daily-note fallback and reused open daily tabs.
  - **`b20a6cf fix: center daily Pomodoro jumps on target editor` — the regression-introducing commit** (see Diagnosis).
- Open SDD tale `sdd/tales/202606/obsidian_daily_jump_scroll.md` (status: `wip`) is the design that landed in `b20a6cf`.
  Its design said to center "the CodeMirror 6 `EditorView` from the provided **Obsidian editor**" — that assumption is
  exactly what breaks the local path (the local path is not handed an Obsidian editor; see Diagnosis).

## Root-Cause Diagnosis

### How the two call paths differ in what `cm` is

`scheduleCenterOnLine(cm, line)` is reached from `jumpToPomodoroTarget(cm, target)`, which has two callers:

1. **Local path** — `jumpToCurrentPomodoro(cm)` is a codemirror-vim action:
   `vim.defineAction("bobLedgerJumpToCurrentPomodoro", (cm) => this.jumpToCurrentPomodoro(cm))`. Here `cm` is the
   **CodeMirror 5 vim adapter** (`window.CodeMirrorAdapter` instance), not an Obsidian `Editor`. Confirmed by the
   CM5-only API the code already calls on it: `getEditorLines(cm)` uses `cm.getValue` / `cm.getLine` / `cm.firstLine` /
   `cm.lastLine` / `cm.lineCount`, and `setEditorCursor`/`scrollEditorIntoView` use `cm.setCursor` /
   `cm.scrollIntoView`. The CM5 adapter exposes its underlying CM6 view as `cm.cm6`.

2. **Daily fallback path** — `openDailyFallbackAndJump()` calls `jumpToPomodoroTarget(view.editor, target)`. Here `cm`
   is the Obsidian `Editor`, whose CM6 `EditorView` is `view.editor.cm`.

### What `b20a6cf` changed

`b20a6cf` added `getEditorViewFromEditor(cm)` and rewrote `scheduleCenterOnLine` to "prefer the editor that was actually
jumped." The new helper resolves the CM6 view as:

```js
function getEditorViewFromEditor(cm) {
  const editorView = cm && (cm.cm || cm); // Obsidian editor (.cm) or a raw EditorView
  if (!editorView || !editorView.state || !editorView.state.doc || typeof editorView.dispatch !== "function")
    return null;
  return editorView;
}
```

This recognizes the Obsidian `Editor` (`.cm`) and a raw `EditorView`, but **not the CM5 vim adapter** (whose view is at
`.cm6`). So for the **local path**, `getEditorViewFromEditor(cm)` returns `null`.

The rewritten `scheduleCenterOnLine` then does, per deferred attempt:

```js
const targetEditorView = getEditorViewFromEditor(cm); // null for the vim adapter
if (centerEditorViewOnPosition(targetEditorView, line, 0)) return; // no-op, returns false
if (!targetEditorView && attempt + 1 < attempts) {
  /* retry next frame */ return;
}
// final attempt:
const activeView = getActiveMarkdownView(this.app);
const activeEditorView = activeView?.editor ? getEditorViewFromEditor(activeView.editor) : null;
const activeCentered = centerEditorViewOnPosition(activeEditorView, line, 0); // succeeds: centers the active view
if (!activeCentered || !activeView || activeView.editor !== cm) {
  // activeView.editor !== cm is ALWAYS true
  scrollEditorIntoView(cm, line, 0); // CM5 "nearest" scroll → CLOBBERS center
}
```

### The two defects, both on the local path

1. **Wrong editor resolution:** because `getEditorViewFromEditor(cm)` cannot see the vim adapter's `.cm6`,
   `targetEditorView` is `null`, so the fast/direct center never happens and the code wastes all 5 deferred frames
   (`CENTER_ON_LINE_ATTEMPTS = 5`) before falling back to the active view.

2. **Center gets clobbered (the visible bug):** on the final attempt the active view _is_ centered correctly, but the
   guard `activeView.editor !== cm` compares an Obsidian `Editor` against the vim adapter. These are different objects
   even though they wrap the **same** underlying editor, so the condition is always true and
   `scrollEditorIntoView(cm, line, 0)` runs. The vim adapter has a real `scrollIntoView`, which performs a "nearest"
   scroll _after_ the center dispatch in the same frame — so the line ends up at the viewport edge, not centered.

The known-good `cbe91fa` version avoided both problems: it centered `getActiveEditorView(this.app)` and only fell back
to `scrollEditorIntoView(cm, …)` **when centering failed**. For the local path the active view is the correct editor, so
centering succeeded and the fallback never ran.

The daily path keeps working because it passes an Obsidian `Editor` (`.cm` resolves), so it centers directly and returns
before reaching the clobbering fallback — which is why this regressed only for the local `\\` jump.

## Design

Two complementary changes in `main.js`, scoped to the centering helpers. The first restores correctness; the second
removes the latency and makes "prefer the jumped editor" actually work for the local path too.

### 1. Teach `getEditorViewFromEditor` to resolve the vim adapter's CM6 view

Resolve the underlying `EditorView` from all three shapes the codebase hands it — CM5 vim adapter, Obsidian `Editor`,
and a raw `EditorView`:

```js
const editorView = cm && (cm.cm6 || cm.cm || cm);
```

- Vim adapter → `cm.cm6` (the CM6 `EditorView`).
- Obsidian `Editor` → `cm.cm` (it has no `.cm6`).
- Raw `EditorView` → itself.

The existing `.state` / `.state.doc` / `.dispatch` validation still rejects anything that is not a usable view, so each
candidate is verified before use. **Verify at implementation time** that an Obsidian vim action's `cm` exposes the CM6
view as `.cm6` (a one-line `console.log(Object.keys(cm))` in the action during manual testing, or inspecting the bundled
codemirror-vim adapter). If the property differs, adjust this single accessor — the rest of the design is independent of
the exact name because of change (2).

### 2. Make `scheduleCenterOnLine` never overwrite a successful center

Restore the `cbe91fa` invariant — _a successful center is the last word; the CM5 nearest-scroll is only a fallback for
when no `EditorView` is reachable_ — while keeping `b20a6cf`'s good ideas (prefer the jumped editor; bounded retry for a
just-activated daily tab whose view lags a frame). Replace the per-attempt body with:

```js
const runAttempt = (attempt) => {
  this.pendingCenterDeferred = null;

  // Prefer the editor that actually received the cursor move (vim adapter for
  // local jumps, the daily Editor for the fallback). Its CM6 view may lag by a
  // frame right after a daily tab is activated.
  const targetEditorView = getEditorViewFromEditor(cm);
  if (centerEditorViewOnPosition(targetEditorView, line, 0)) {
    return;
  }

  // Give the jumped editor a bounded number of frames to attach its view
  // before consulting the (possibly stale) active Markdown view.
  if (!targetEditorView && attempt + 1 < attempts) {
    this.pendingCenterDeferred = deferToNextFrame(() => runAttempt(attempt + 1));
    return;
  }

  // Fallback: center the active Markdown view if reachable; only if no view can
  // be centered do we issue the CM5 "nearest" scroll on the handed editor.
  if (centerEditorViewOnPosition(getActiveEditorView(this.app), line, 0)) {
    return;
  }
  scrollEditorIntoView(cm, line, 0);
};
```

Key differences from the broken version:

- The clobbering guard `if (!activeCentered || !activeView || activeView.editor !== cm) scrollEditorIntoView(...)` is
  gone. The CM5 nearest-scroll now runs **only** when neither the jumped editor nor the active view could be centered —
  it can never overwrite a center that already succeeded.
- With change (1), the local path resolves `targetEditorView` on the **first** deferred frame (via `cm.cm6`), centers
  the exact editor the cursor moved in, and returns — no wasted frames, no active-view round-trip.
- The daily path is unchanged in spirit: it centers the passed daily `Editor` directly; if that editor's CM6 view is not
  ready yet, the bounded retry waits for it before consulting the (possibly stale) active view, so a stale active editor
  is never centered while a valid target view is pending.

### Why this satisfies the daily tale's guarantees too

The `obsidian_daily_jump_scroll` verification items still hold: deferred centering uses the daily editor's own CM6 view
before consulting `getActiveViewOfType()`; a stale active editor is not centered when the target view is available
(target is tried first and, when not yet attached, we retry rather than center the active view); and the CM5
`editor.scrollIntoView` remains the final fallback when no CM6 view exists.

### Scope guardrails

- Edit **only** `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- Do not change the `\\`, `\\p`, `\\P`, `\\o`, `\\O` mappings, target selection (`getJumpPomodoroTarget`), notices, the
  daily-notes config, templates, `.obsidian.vimrc`, hotkeys, manifest, or any other plugin.
- Keep `CENTER_ON_LINE_ATTEMPTS` and the `options.attempts` override.

## Verification

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-ledger-tools/main.js
```

Focused Node helper checks (stub Obsidian objects; helpers are exported on `module.exports.helpers`):

- `getEditorViewFromEditor` returns the CM6 view for each shape: `{ cm6: view }` (vim adapter), `{ cm: view }` (Obsidian
  editor), and a raw `view`; returns `null` for objects with no usable view.
- A vim-adapter-shaped `cm` (`{ cm6: fakeView, scrollIntoView }`) drives a single `dispatch` with a **center**
  `scrollIntoView` effect and triggers **no** subsequent `scrollIntoView` (nearest) call — i.e. no clobber.
- When the passed editor has no CM6 view on frame 0 but the active view does, centering still resolves to a centered
  dispatch within the bounded retry, and the CM5 nearest fallback is not used.
- When no CM6 view is reachable anywhere, `scrollEditorIntoView(cm, line, 0)` runs exactly once as the final fallback.
- Daily fallback unchanged: existing-tab activation does not invoke the Daily Notes command; no-open-daily still does;
  the active-daily/no-target guard still avoids a self-reopen loop.

Manual live acceptance (if a GUI Obsidian session is available):

- In a note with a `## Pomodoros` target, scroll the target line to the top/bottom of the viewport, press `\\`, and
  confirm the line snaps to the **middle** (regression check).
- Repeat for the daily fallback: from a note with no target, press `\\`, confirm the daily tab activates, the cursor
  lands on the daily Pomodoro line, and it is **centered**.
- Confirm the active-daily/no-target case still only shows the notice and does not reopen.

## Finalization

- Inspect `git -C /home/bryan/bob status` before editing and before commit; preserve all unrelated dirty/untracked vault
  files (including today's daily note).
- Commit only `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js` via `/sase_git_commit`.
- Report any checks that could not be run, especially live GUI acceptance and the `.cm6` property confirmation.
