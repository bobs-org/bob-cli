---
create_time: 2026-06-03 22:52:57
status: wip
prompt: sdd/prompts/202606/obsidian_backslash_daily_fallback.md
---
# Obsidian `\\` Daily Fallback Plan

## Goal

Improve the Bob Obsidian Vim normal-mode `\\` mapping so it keeps the current Pomodoro jump behavior when the active
file has a ledger/Pomodoro target, but falls back to today's daily note when:

- the current active Markdown file is not today's daily file; and
- the current file has no Pomodoro ledger line that `\\` can jump to.

The intended user experience is:

1. Press `\\` inside a note with a usable `## Pomodoros` target: jump to that line and keep the existing centered-scroll
   behavior.
2. Press `\\` inside any other note with no local target: open today's daily file. After the daily file opens, jump to
   and center its active/last-completed Pomodoro line if one exists.
3. Press `\\` while already focused on today's daily file with no target: do not loop or reopen the same file; show the
   existing-style "no Pomodoro line" notice.

## Context Reviewed

- Project short memory says this is an ephemeral `bob-cli_<N>` workspace and build/test commands should stay anchored in
  the workspace clone.
- Obsidian long memory was read through the required audited command:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and vim keymap context before planning changes to the normal-mode backslash mapping"`.
- The live vault is `/home/bryan/bob`, and its `AGENTS.md` requires checking `git status` before edits, preserving
  unrelated dirty/synced files, and committing task-related vault edits with `/sase_git_commit` before terminating after
  implementation changes.
- Vault status currently has unrelated dirty note/content files, including 2026 daily notes, `obsidian.md`,
  `obsidian_ref.md`, `sase.md`, and several untracked reference files. The target plugin file is not listed dirty.
- The `bob-cli` workspace is currently clean.
- The mapping lives in `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`, not in `hotkeys.json`:
  `vim.mapCommand("\\\\", "action", "bobLedgerJumpToCurrentPomodoro", {}, { context: "normal" })`.
- Current `jumpToCurrentPomodoro(cm)`:
  - reads the active editor lines;
  - calls `getJumpPomodoroTarget(lines)`;
  - if a target exists, sets the cursor and calls `scheduleCenterOnLine(...)`;
  - if no target exists, immediately shows `No ## Pomodoros section found` or
    `No active or completed Pomodoro line found`.
- `getJumpPomodoroTarget(lines)` already defines the local jump contract: prefer the active open Pomodoro item in the
  Pomodoros section, otherwise fall back to the last completed Pomodoro item.
- The earlier centering fix is already present: `scheduleCenterOnLine` defers CM6 centering until after the Vim command
  cycle, with a CM5 `scrollIntoView` fallback.
- The vault daily note config is `/home/bryan/bob/.obsidian/daily-notes.json`: `format: "YYYY/YYYYMMDD[_day]"`,
  `template: "_templates/daily"`.
- The configured Obsidian command id for the Daily Notes core plugin is `daily-notes`; it is already bound in
  `hotkeys.json`.
- `/home/bryan/bob/_templates/daily.md` contains a `## Pomodoros` section and a starter `- [ ] ()` placeholder, so a
  newly-created daily note should normally have a jump target.

## Product Decisions

1. Preserve local-first behavior.
   - `\\` should not leave the current file if the current file has a valid target according to the existing
     `getJumpPomodoroTarget` rules.
   - This keeps non-daily files with their own Pomodoro sections usable.

2. Treat "no ledger/Pomodoro line to jump to" as "no target from `getJumpPomodoroTarget`."
   - This covers missing `## Pomodoros` section, an empty section, and a section with no open placeholder/timed line and
     no completed placeholder/timed line.

3. Use today's daily note as the fallback destination only when it is not already focused.
   - Compute today's configured daily path and compare it to `app.workspace.getActiveFile()?.path`.
   - If the active file already is today's daily file, show the current no-target notice instead of reopening it.

4. Open/create the daily note through Obsidian's Daily Notes command where possible.
   - Prefer `app.commands.executeCommandById("daily-notes")` so the existing Daily Notes and Templater workflow handles
     path, folder creation, and template application.
   - Keep a small path-based fallback for environments where the command is unavailable: resolve or create the
     configured `YYYY/YYYYMMDD_day.md` note conservatively.

5. After opening the daily note, attempt the same Pomodoro jump there.
   - Wait until the daily note is active, then read the active Markdown editor lines.
   - If the daily note has a target, set the cursor and reuse `scheduleCenterOnLine` so the existing centering behavior
     stays consistent.
   - If the daily note opens but has no target, leave it focused and show a notice such as
     `No active or completed Pomodoro line found`.

## Implementation Scope

Expected implementation file:

- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`

No `bob-cli` Rust or script changes are expected.

Likely changes:

- Make the Vim action tolerate the asynchronous fallback path:
  - `vim.defineAction("bobLedgerJumpToCurrentPomodoro", (cm) => this.jumpToCurrentPomodoro(cm))` can continue to call
    the method; the method may return a promise after scheduling/opening work, and the Vim command does not need to
    await it for the local synchronous path.
- Refactor `jumpToCurrentPomodoro(cm)` into a local-target branch plus fallback branch:
  - local target: keep current cursor set + deferred center logic unchanged;
  - no local target: call a new `openDailyFallbackAndJump(error, cm)` helper.
- Add helpers for daily behavior:
  - `todayDailyPath(now, dailyOptions)` for the configured Bob daily-note format, with a default of
    `YYYY/YYYYMMDD_day.md`;
  - `isTodayDailyFile(app, file)` or equivalent path comparison;
  - `openTodayDailyNote(app)` that prefers `app.commands.executeCommandById("daily-notes")` and falls back to
    `workspace.getLeaf(false).openFile(file)` for an existing path;
  - optional `waitForActiveMarkdownView(...)` / deferred retry helper so the post-open jump runs after Obsidian has
    attached the editor.
- Reuse existing helpers rather than inventing a second Pomodoro parser:
  - `getEditorLines`;
  - `getJumpPomodoroTarget`;
  - `setEditorCursor`;
  - `scheduleCenterOnLine`;
  - `getActiveEditorView`.
- Keep notices minimal and action-oriented:
  - current file is today's daily and has no target: existing no-target message;
  - daily command/file open fails: `Could not open daily note`;
  - daily opens but has no target: existing no-target message.
- Export pure helper functions under `module.exports.helpers` when useful for focused Node checks.

Implementation guardrails:

- Touch only `bob-ledger-tools/main.js` unless implementation inspection proves a manifest/config file must change.
- Do not modify dirty daily notes or other unrelated vault content.
- Re-check `git -C /home/bryan/bob status --short` before and after edits.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
jq '.' /home/bryan/bob/.obsidian/daily-notes.json
```

Focused Node helper checks with stubbed `obsidian`, `@codemirror/state`, and `@codemirror/view` modules:

- `todayDailyPath(new Date(...))` returns `YYYY/YYYYMMDD_day.md` for the Bob daily-note format.
- Active file path equal to today's daily path suppresses the fallback and preserves the no-target notice behavior.
- Active file path different from today's daily path plus no local target invokes the daily open path.
- A current-file Pomodoro target still takes precedence and does not invoke the daily open path.
- After a daily note open, the helper reads the active editor, finds the daily Pomodoro target, sets the cursor, and
  schedules the existing center behavior.
- Daily-note open failure returns `false` and shows the daily open failure notice without throwing.

Manual live-vault acceptance checks:

- In a non-daily note with no `## Pomodoros` target, press `\\` in Vim normal mode and confirm today's daily note opens.
- If today's daily note has a Pomodoro placeholder or active/completed line, confirm the cursor lands there and the line
  is vertically centered.
- In a non-daily note that has its own Pomodoro target, press `\\` and confirm it jumps locally instead of opening the
  daily note.
- In today's daily note, remove or avoid a jump target only if safe to test without editing user content; otherwise use
  a scratch note/stub. Confirm no self-reopen loop occurs when no target exists.

Optional workspace checks if no Rust code is touched:

```bash
git status --short
git -C /home/bryan/bob status --short
```

## Finalization

- If implementation edits `/home/bryan/bob`, commit only the task-related vault plugin file with the required
  `/sase_git_commit` workflow before finishing.
- Leave unrelated dirty/synced note files untouched and call them out in the final status.
- Report any validation that could not be run, especially live Obsidian manual checks if no GUI session is available.

## Risks

- `app.commands.executeCommandById("daily-notes")` may return before the editor is fully ready. Mitigation: defer or
  retry the post-open Pomodoro jump against the active Markdown view.
- The Daily Notes command may be disabled/unavailable in a test stub or future config. Mitigation: keep a path-based
  fallback using the configured `YYYY/YYYYMMDD[_day]` format.
- Manually creating a daily note could bypass Templater processing. Mitigation: prefer the Daily Notes command and only
  create manually as a last-resort fallback if the command cannot run.
- Automated Node checks cannot fully prove Obsidian command timing. Mitigation: keep the fallback small and finish with
  a live-vault manual checklist.
