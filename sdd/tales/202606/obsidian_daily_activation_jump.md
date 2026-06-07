---
create_time: 2026-06-07 06:34:21
status: wip
prompt: sdd/prompts/202606/obsidian_daily_activation_jump.md
---
# Plan: Fix Obsidian Daily Tab Activation Jump

## Goal

Fix the Bob Ledger Tools `\\` daily-note fallback so an already-open daily note tab is not only activated visually, but
also reliably used as the editor source for the Pomodoro jump.

The desired behavior remains:

- If the current note has an active or completed Pomodoro target, jump locally.
- If the current note has no target and today's daily note is already open, activate that existing tab and jump to the
  daily Pomodoro target.
- If today's daily note is not already open, keep the Daily Notes command and open/create fallback.
- If today's daily note is already active and has no target, keep the existing no-target notice and avoid reopen loops.

## Context Reviewed

- Project instructions and short memory confirm this is an ephemeral `bob-cli_<N>` workspace.
- Required Obsidian long memory was read with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault workflow constraints before diagnosing and fixing daily note tab activation behavior"`.
- Vault instructions in `/home/bryan/bob/AGENTS.md` require inspecting vault status before edits, preserving unrelated
  dirty files, and committing only task-related vault edits with `/sase_git_commit` before terminating after changes
  under `~/bob`.
- Prior approved plan: `sdd/tales/202606/obsidian_backslash_daily_open_tab.md`.
- Current plugin file: `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- The plugin file is clean at commit `e568a7d`; the vault has unrelated dirty files that must remain untouched.

## Diagnosis

The likely root cause is in the new activation path:

- `openTodayDailyNote()` finds the existing daily-note leaf with `findOpenMarkdownLeafByPath()`.
- `activateMarkdownLeaf()` calls `workspace.revealLeaf(leaf)` or `workspace.setActiveLeaf(leaf, { focus: true })`.
- After activation, it returns only `waitForActiveMarkdownView(app, { path: dailyPath })`.
- `waitForActiveMarkdownView()` verifies activation indirectly through `workspace.getActiveViewOfType(MarkdownView)`.

The user's symptom says the tab is activated, but then the operation reports failure and does not jump. That is exactly
what happens if Obsidian reveals the target leaf visually, while `getActiveViewOfType(MarkdownView)` does not return the
expected daily view within the polling window. In that case the code discards the known matching leaf, returns `null`,
and `openDailyFallbackAndJump()` never reads the already-activated leaf's editor.

The implementation should not require an active-view lookup to rediscover a leaf that was already matched by path before
activation. The activation helper should be able to validate and return the known leaf's own Markdown view after
revealing it.

## Design

1. Keep the existing local-first command flow.
   - Do not change Pomodoro target detection.
   - Do not navigate away when the current editor has a valid local target.

2. Add a path-aware Markdown view helper for a leaf.
   - Reuse `getMarkdownLeafFile(leaf)` to confirm the leaf is a Markdown leaf.
   - Add a helper such as `getMarkdownLeafViewByPath(leaf, path)` that returns `leaf.view` when it is a Markdown view
     with an editor and its file path matches the expected daily path.
   - Export the helper for focused Node checks.

3. Make activation return the matched leaf's view when possible.
   - After `revealLeaf` / `setActiveLeaf`, first check the known leaf directly with the new helper.
   - If the editor is not attached immediately, poll the known leaf for a short bounded interval.
   - Keep `waitForActiveMarkdownView()` as a secondary fallback, since it still works for command/open-file paths and
     may work for activation in some Obsidian layouts.
   - Preserve null return only when neither the active view nor the matched leaf exposes a usable Markdown editor.

4. Keep fallback behavior unchanged.
   - If no matching open daily leaf exists, continue to use the Daily Notes command first.
   - If the command path fails to activate the expected daily file, continue to resolve/create and `openFile`.
   - Keep existing user-facing notices unless changing one is necessary to reflect the actual failure.

5. Avoid unrelated churn.
   - Edit only `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
   - Do not touch vault notes, hotkeys, vimrc, plugin manifests, daily-note config, or memory files.

## Verification

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
jq '.' /home/bryan/bob/.obsidian/daily-notes.json
```

Focused Node helper checks with stubbed Obsidian objects:

- Existing daily leaf activates and returns its own view even when `workspace.getActiveViewOfType()` keeps returning the
  previous non-daily view.
- Existing daily leaf activation does not invoke the Daily Notes command.
- Known leaf polling handles delayed editor attachment.
- Fallback to active-view polling still works when the active view does become the daily note.
- No-open-leaf path still invokes the Daily Notes command/open-file fallback.
- `openDailyFallbackAndJump()` reads the returned daily editor, finds the Pomodoro target, sets the cursor, and
  schedules centering.
- Active-daily/no-target guard still avoids a self-reopen loop.

Status checks:

```bash
git -C /home/bryan/bob status --short -- .obsidian/plugins/bob-ledger-tools/main.js
git -C /home/bryan/bob status --short
```

Manual live acceptance, if a GUI Obsidian session is available:

- Open today's daily note in one tab.
- Focus another note with no Pomodoro target.
- Press `\\`.
- Confirm Obsidian switches to the existing daily tab and jumps to the Pomodoro target line.

## Finalization

- Re-check vault status before editing and before commit.
- Commit only `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js` through the required `sase_git_commit`
  workflow after source changes.
- Report any checks that could not be run, especially live GUI acceptance.
