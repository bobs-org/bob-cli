---
create_time: 2026-06-11 15:01:43
status: wip
prompt: sdd/prompts/202606/pomodoro_next_cursor_jump.md
---
# Plan: Jump Cursor to the Next Pomodoro Task After `<Ctrl+Enter>` Completion

## Goal

Extend the Pomodoro `<Ctrl+Enter>` completion flow (committed as
`27188d3 feat: complete pomodoro carry-forward on ctrl-enter`) so that, after the flow finishes, the cursor jumps to the
line where the **next Pomodoro task** is defined:

- the newly inserted `- [ ] ()` placeholder line, when the completed Pomodoro was the last one and a new Pomodoro had to
  be created; or
- the pre-existing next Pomodoro task line, when one already existed below.

Today the cursor stays parked on the just-completed Pomodoro line. After this change the user lands directly on the
Pomodoro they will work on next — ready to fill in its time range or start on its sub-bullet tasks.

## Context Reviewed

- Read `memory/short/sase.md` (ephemeral `bob-cli_<N>` workspace; no sibling repos).
- Long-term memory: this workspace has only `memory/long/cli_rules.md`, which applies to new `bob-cli`
  subcommands/options. This task changes only the vault Obsidian plugin (no Rust CLI changes), so it is not required.
- Read `/home/bryan/bob/AGENTS.md`: inspect `git status` before editing, never touch unrelated pre-existing dirty files,
  commit only this task's vault changes via `/sase_git_commit` before finishing. The vault currently has a large set of
  unrelated dirty files (sync churn across `2023/*.md`, `.obsidian/community-plugins.json`, etc.); the target file
  `.obsidian/plugins/task-status-cycler/main.js` is currently **clean** at commit `27188d3`.
- Re-inspected `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js` (the Pomodoro completion flow) and
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js` (existing cursor/jump/snippet conventions).
- Prior plan for the base behavior: `sdd/tales/202606/pomodoro_ctrl_enter_close_and_create.md` (explicitly decided "the
  cursor stays on the current (now completed) Pomodoro line" — this task supersedes that decision).

## Current State

- `completeActivePomodoroTask()` orchestrates the flow: completes transcluded sub-bullet sources, then calls
  `buildPomodoroCompletionPlan(lines, section, pomodoroLine)` (pure, exported for tests) and applies the returned edits
  via `applyPomodoroCompletionPlan(editor, plan, cursor)`.
- `buildPomodoroCompletionPlan` already returns everything needed to locate the next Pomodoro:
  - `nextPomodoroLine` — the pre-existing next Pomodoro's line, or `null`;
  - `createdPomodoro` — whether a `- [ ] ()` placeholder is being inserted;
  - `sourceRange.endLine` — the insertion point of that placeholder (first line after the completed Pomodoro's
    sub-bullet block).
- `applyPomodoroCompletionPlan` applies the edits bottom-up, then restores the cursor to `plan.pomodoroLine` with the
  pre-edit `ch` clamped to the new line length. This cursor restore is the only behavior this task changes.
- Relevant vault conventions in `bob-ledger-tools/main.js`:
  - Its jump-to-Pomodoro command places the cursor at **column 0** of the target Pomodoro line.
  - Its `se` time-range snippet (`ledgerRange` trigger) is designed to be typed **inside empty parens**: the expansion
    explicitly merges with a surrounding `(`/`)` pair (`line[fromCh - 1] === "(" && line[cursorCh] === ")"`), replacing
    `(se)` with the filled time range. Its placeholder detection regex is `/\(\s*\)/`.

## Product Decisions

1. **The jump happens whenever the Pomodoro-completion flow runs** — both when a new placeholder is created and when a
   next Pomodoro already existed. The user request ("after creating that Pomodoro task, if necessary") makes creation
   conditional, not the jump.
2. **The target is the Pomodoro task line itself**, never one of its sub-bullets — including the
   created-placeholder-with-empty-sub-bullet case (`- [ ] ()` followed by `\t- `): the cursor lands on the `- [ ] ()`
   line.
3. **Cursor column is placeholder-aware**:
   - If the target line is a placeholder Pomodoro (empty parens, mirroring the ledger plugin's `/\(\s*\)/` shape), the
     cursor lands **between the parens** (on the `)` in Vim normal mode). The user's natural next action is filling in
     the time range, and the `se` snippet is built for exactly this position: press `i`, type the trigger, expand.
   - Otherwise (a timed pre-existing Pomodoro), the cursor lands at **column 0**, matching the ledger plugin's existing
     jump-to-Pomodoro convention.
4. **Scrolling**: best-effort `editor.scrollIntoView` on the target position when the editor supports it, so a
   placeholder created at the bottom edge of the viewport is not left off-screen. Guarded the same defensive way as the
   existing cursor calls; absence of the API never aborts the flow.
5. **All other paths are untouched**: re-opening a done Pomodoro (plain toggle), `<Ctrl+Enter>` on non-Pomodoro task
   lines, transcluded sub-bullet lines, files without a `## Pomodoros` section, the command-palette command, and every
   other keymap keep today's behavior, including their existing cursor handling.

## Implementation Approach

Single vault file to edit: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.

1. **`buildPomodoroCompletionPlan` returns the post-edit cursor target line.**
   - Add `cursorTargetLine` to the returned plan object:
     - `createdPomodoro === true` → `sourceRange.endLine` (the placeholder's post-edit line: the insertion happens at
       that line and every other edit touches lines at or above the completed Pomodoro, so the number is stable);
     - otherwise → `nextPomodoroLine` (the only possible insertion goes at the next Pomodoro's sub-bullet block end,
       strictly below it, so its line number is also stable in post-edit coordinates).
   - Pure and unit-testable; no editor involvement.
2. **New pure helper `getPomodoroCursorTargetCh(lineText)`** (exported on `module.exports.helpers`):
   - If the line contains an empty-parens placeholder pair (local regex mirroring the ledger plugin's `/\(\s*\)/`,
     consistent with how this plugin already mirrors `POMODOROS_HEADING_RE` and the `- [ ] ()` literal), return the
     column just before the `)`.
   - Otherwise return `0`.
3. **`applyPomodoroCompletionPlan` jumps instead of restoring.**
   - Replace the cursor-restore block: when a cursor existed and the editor supports `setCursor`, set the cursor to
     `plan.cursorTargetLine` with `ch` from `getPomodoroCursorTargetCh(<post-edit line text>)`, clamped to the line
     length (same clamping pattern as today). Fall back to today's behavior (`plan.pomodoroLine`) only if
     `cursorTargetLine` is somehow missing (defensive, e.g. a stale plan shape).
   - Add the guarded best-effort `scrollIntoView` call after `setCursor`.
4. **No wiring changes**: `handleVimTaskToggleOpenDone`, `getActivePomodoroTaskContext`, and
   `completeActivePomodoroTask` keep their current structure; the change is contained in the plan builder and the apply
   step.

No expected edits to `bob-ledger-tools/main.js`, `bob-navigation-hotkeys/main.js`, `.obsidian/hotkeys.json`,
`.obsidian.vimrc`, any vault notes, or any `bob-cli` Rust/script/test/README/memory files.

## Acceptance Criteria

Starting from the cursor on the last open Pomodoro:

```md
- [ ] (**1205-1230** [t:: 25m])
  - [[bob#^close-and-create-pom-task]]
```

pressing `<Ctrl+Enter>` still produces the same buffer text as today (completed line, new `- [ ] ()`, carried-forward
bullets), and additionally:

- the cursor now sits on the new `- [ ] ()` line, between the parens — pressing `i` and typing the `se` snippet trigger
  immediately fills the time range;
- when a next Pomodoro already existed (timed), the cursor sits at column 0 of that pre-existing Pomodoro line, which
  keeps its line number even when carried-forward bullets are appended under it;
- when the pre-existing next Pomodoro is itself a placeholder `- [ ] ()`, the cursor lands between its parens;
- when a new placeholder is created with the single empty `\t- ` sub-bullet, the cursor is on the placeholder line, not
  the sub-bullet;
- `<Ctrl+Enter>` on a done Pomodoro (reopen), on non-Pomodoro tasks, on transcluded sub-bullet lines, and in files
  without a `## Pomodoros` section keeps today's cursor behavior exactly.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused Node checks (same fake-editor approach as the prior `<Ctrl+Enter>` work, driving the exported helpers):

- `buildPomodoroCompletionPlan().cursorTargetLine`: last-Pomodoro/created-placeholder case → `sourceRange.endLine`;
  existing-next case → `nextPomodoroLine`, including with dedupe-filtered bullet insertion below it.
- `getPomodoroCursorTargetCh`: `- [ ] ()` → between parens; `- [ ] ( )` (spaced) → before `)`; timed Pomodoro line →
  `0`; defensive on empty/non-string input.
- Fake-editor runtime check through `completeActivePomodoroTask`: after the full flow, `getCursor()` reports the
  placeholder line + between-parens column (created case) and the next-Pomodoro line + column 0 (pre-existing case);
  buffer text identical to the pre-change behavior; reopen path leaves the cursor where the plain toggle puts it.

Manual smoke test in the vault after a plugin reload, on a scratch daily-style note: complete the last Pomodoro (cursor
should land between the new placeholder's parens and the `se` snippet should expand there), complete a Pomodoro with a
following one (cursor lands on it at column 0), reopen a done Pomodoro (no jump).

Vault hygiene before finishing:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js
```

Commit only `.obsidian/plugins/task-status-cycler/main.js` (plus any scratch-note cleanup) via the `/sase_git_commit`
workflow, leaving the many unrelated pre-existing dirty vault files alone.

## Risks

- **Post-edit line math**: the target line is computed in post-edit coordinates from the same `lines` snapshot the edits
  are built from, with no `await` between build and apply, and both possible insertions land strictly below the target
  line — so no shift can occur. The focused fake-editor checks assert the final cursor line against the final buffer to
  catch any regression here.
- **Vim normal-mode column clamping**: CodeMirror's Vim mode clamps the normal-mode cursor to the last character of the
  line; for `- [ ] ()` the between-parens column _is_ the last character (`)`), so no drift. The `ch` is also clamped to
  the actual post-edit line length, as the existing code already does.
- **Async cursor timing**: the flow already calls `setCursor` at the end of the same async orchestration today (to the
  completed line) without issues; only the target changes, so no new interaction with Vim mode or Obsidian focus
  handling is introduced.
