---
create_time: 2026-06-07 06:46:08
status: wip
prompt: sdd/prompts/202606/obsidian_daily_jump_scroll.md
---
# Plan: Fix Daily Pomodoro Jump After Tab Activation

## Goal

Fix the Bob Ledger Tools `\\` daily-note fallback so it not only activates today's already-open daily note tab, but also
places the cursor and viewport on the Pomodoro ledger target line in that daily note.

Expected behavior:

- If the current editor has an active or completed Pomodoro target, keep jumping locally.
- If the current editor has no target and today's daily note is already open, activate that existing daily tab and jump
  to the daily Pomodoro target line.
- If today's daily note is not open, keep the Daily Notes command/open-file fallback.
- If today's daily note is already active and has no target, keep the existing no-target notice and avoid reopen loops.

## Context Reviewed

- Required Obsidian long-term memory was read with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and plugin workflow context before planning a fix for daily ledger jump behavior"`.
- `/home/bryan/bob/AGENTS.md` says the vault is actively synced, unrelated dirty files must be preserved, and any
  implementation changes under `~/bob` must be committed through `sase_git_commit`.
- Current plugin file: `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- The target plugin file is currently clean.
- The vault has unrelated dirty/untracked files, including today's daily note; these must not be staged, reverted, or
  rewritten as part of the plugin fix.
- Previous fix commit: `d793951 fix: use matched daily tab view for Pomodoro jumps`.
- Today's daily-note config resolves to `2026/20260607_day.md`.
- Today's `## Pomodoros` section currently includes one completed entry and an open placeholder `- [ ] ()`. Based on the
  current parser, `getJumpPomodoroTarget()` should choose that placeholder line, so the remaining failure is more likely
  navigation/scroll timing than target selection.

## Diagnosis

The last fix deliberately stopped depending solely on `workspace.getActiveViewOfType(MarkdownView)` after activating an
already-open daily leaf. That solved the failure notice, but it exposed a second stale-active-view dependency:

- `openTodayDailyNote()` can now return the known daily leaf's own Markdown view even while Obsidian's active-view API
  still reports the previous note for a short time.
- `openDailyFallbackAndJump()` reads the returned daily editor and calls `jumpToPomodoroTarget(view.editor, target)`.
- `jumpToPomodoroTarget()` sets the cursor on the passed editor, but `scheduleCenterOnLine()` later calls
  `centerEditorViewOnPosition(getActiveEditorView(this.app), line, 0)`.
- If `getActiveEditorView(this.app)` is still the previous note during that deferred frame, the function can scroll the
  wrong editor and return success, preventing the fallback `scrollEditorIntoView(cm, line, 0)` from running on the daily
  editor.

That failure mode matches the symptom: the error is gone and the daily tab can be activated, but the visible editor does
not jump to the correct ledger line.

Secondary possibilities to guard against during implementation:

- The daily editor may expose an Obsidian `Editor` before its underlying CodeMirror 6 view is fully ready.
- The cursor move may need to happen after the activated leaf/editor receives focus.
- If a future daily note has multiple open timed entries, target selection should remain unchanged unless tests prove
  the parser is selecting the wrong line.

## Design

1. Keep target selection unchanged unless reproduction proves it is wrong.
   - Preserve `getJumpPomodoroTarget()` semantics: open timed Pomodoro first, then first open placeholder, then last
     completed Pomodoro.
   - Add a focused check using today's Pomodoro section to prove the placeholder line is the selected target.

2. Make centering target the editor that was actually jumped.
   - Add a helper such as `getEditorViewFromEditor(editor)` that safely returns the CodeMirror 6 `EditorView` from the
     provided Obsidian editor when available.
   - Update `scheduleCenterOnLine()` to prefer the passed editor's own `EditorView` over the global active Markdown
     view.
   - Keep the existing active-view fallback for cases where a direct editor view is unavailable.
   - Keep `scrollEditorIntoView(editor, line, 0)` as the final fallback, but make sure a successful scroll on a stale
     active editor cannot suppress scrolling the intended editor.

3. Make daily fallback navigation tolerant of activation timing.
   - After `openTodayDailyNote()` returns a daily view, perform the cursor move against `view.editor`, not a re-resolved
     active editor.
   - If the direct editor view is not ready on the first frame, retry centering for a short bounded number of animation
     frames/timeouts before falling back to Obsidian editor scrolling.
   - If the editor has a safe focus method, focus the daily editor/leaf before or immediately after setting the cursor.
   - Avoid adding broad sleeps to the command path; use bounded polling only around editor-view readiness.

4. Keep file scope narrow.
   - Implementation should edit only: `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
   - Do not touch daily notes, templates, `.obsidian.vimrc`, hotkeys, manifests, memory files, or unrelated plugin
     files.

5. Preserve current user-facing behavior.
   - Keep existing notices unless a failure path becomes materially different.
   - Do not reopen today's daily note when it is already active and has no target.
   - Do not change the `\\`, `\\p`, `\\P`, `\\o`, or `\\O` Vim mappings.

## Verification

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
jq '.' /home/bryan/bob/.obsidian/daily-notes.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-ledger-tools/main.js
```

Focused Node helper checks with stubbed Obsidian objects:

- Today's Pomodoro section target resolves to the open placeholder line after the completed entry.
- Local `jumpToPomodoroTarget()` still sets the cursor and schedules centering for the current editor.
- Daily fallback sets the cursor on the returned daily editor when the active Markdown view remains the previous note.
- Deferred centering uses the daily editor's own CodeMirror view before consulting `getActiveViewOfType()`.
- A stale active editor is not scrolled when a valid target editor view is available.
- If the direct CodeMirror view is initially unavailable, bounded retry eventually centers the daily editor once it
  appears.
- If no CodeMirror view is available, fallback `editor.scrollIntoView({ line, ch: 0 })` runs on the intended daily
  editor.
- Existing daily-tab activation still does not invoke the Daily Notes command.
- No-open-daily fallback still invokes the Daily Notes command/open-file path.
- Active-daily/no-target guard still avoids a self-reopen loop.

Manual live acceptance, if a GUI Obsidian session is available:

- Open today's daily note in one tab and scroll away from the `## Pomodoros` placeholder.
- Focus another note with no Pomodoro target.
- Press `\\`.
- Confirm Obsidian switches to the existing daily tab, places the cursor on `- [ ] ()`, and scrolls that line into view.
- Repeat with today's daily note not already open to confirm the command/open-file fallback still jumps.

## Finalization

- Inspect vault status before implementation and before commit.
- Preserve all unrelated dirty/untracked vault files.
- Commit only `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js` through `sase_git_commit` after
  implementation.
- Report any checks that could not be run, especially live GUI acceptance.
