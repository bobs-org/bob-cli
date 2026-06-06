---
create_time: 2026-06-06 07:06:35
status: wip
prompt: sdd/prompts/202606/obsidian_enter_current_line.md
---
# Obsidian Vim Enter Current-Line Plan

## Goal

Make Vim normal-mode bare `<Enter>` in the Bob Obsidian vault target the current cursor line for link open/create
behavior. Preserve counted Enter behavior, fallback movement behavior, Backspace behavior, and Ctrl+Enter task toggling.

## Context Reviewed

- Read the required SASE planning instructions from `/home/bryan/.codex/skills/sase_plan/SKILL.md`.
- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian workflow context before changing vim-mode keymap behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits require checking status, preserving unrelated work, and committing
  task-related vault changes with `/sase_git_commit` before termination.
- Inspected the existing SDD prompt and tale:
  - `sdd/prompts/202606/enter_current_line.md`
  - `sdd/tales/202606/enter_current_line.md`
- Inspected the live vault plugin files:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
  - `/home/bryan/bob/.obsidian.vimrc`
  - `/home/bryan/bob/.obsidian/community-plugins.json`
- Checked current vault status for the relevant files; the two affected plugin files and vimrc/hotkeys files are clean.
  There are unrelated dirty vault note/template files that must be left alone.
- Inspected recent vault history and found `d17289a fix: target current line for bare Enter links`, which introduced the
  intended current-line targeting logic.

## Current Findings

- The current `bob-navigation-hotkeys` source already distinguishes "no Vim repeat/count" from "explicit repeat":
  - `hasVimRepeat(actionArgs)` detects whether CodeMirror Vim supplied a count.
  - `getVimOffsetTargetLine(cm, actionArgs, direction, defaultOffset)` accepts a per-key default offset.
  - `getVimEnterTargetLine(cm, actionArgs)` passes default offset `0`, so bare Enter targets `cursor.line`.
  - `getVimBackspaceTargetLine(cm, actionArgs)` passes default offset `-1`, preserving previous-line Backspace behavior.
- `task-status-cycler` still maps normal-mode `<CR>` to `taskStatusCyclerOpenNextLineLink`, but that action delegates to
  `bob-navigation-hotkeys.handleVimEnterLinkAction(cm, actionArgs)` before using fallback movement.
- Fallback movement still uses normalized repeat `1`, so bare Enter with no actionable link continues to move one line
  down.
- `.obsidian.vimrc` does not map `<Enter>`; it only maps daily note, previous/next labeled link, transclusion toggle,
  and child-note commands. It is unlikely to be overriding `<CR>`.
- A focused Node VM check with minimal `obsidian` and CodeMirror stubs confirms the helper behavior on disk:
  - bare `{}` => current line
  - `null` args => current line
  - `{ repeat: undefined }` => current line
  - `{ repeat: 1 }` => one line down
  - `{ repeat: 5 }` => five lines down
  - bare Backspace => one line up

## Implementation Plan

1. Keep source edits blocked until this plan is submitted with `sase plan`.

2. After plan submission, perform a focused verification pass against the current vault files:
   - Run syntax checks for both custom plugins.
   - Run the Node VM helper check again with stubs and record the results.
   - Run `git -C /home/bryan/bob diff --check` on the relevant plugin/vimrc files.

3. If verification confirms the source behavior remains correct, make no source changes. Treat the user's observed
   behavior as a likely live Obsidian runtime issue:
   - The running Obsidian process may still have the pre-`d17289a` plugin code loaded.
   - The practical fix is to reload Obsidian or disable/enable the `bob-navigation-hotkeys` and `task-status-cycler`
     plugins.
   - Report the exact source-level verification and the reload recommendation.

4. If verification uncovers a mismatch, patch only the minimal affected code:
   - Ensure Enter link targeting calls `handleVimLineLinkAction(cm, actionArgs, 1, 0)`.
   - Ensure the target-line helper treats missing repeat as default offset `0` for Enter.
   - Ensure explicit counts still use `direction * normalizeVimRepeat(actionArgs.repeat)`.
   - Ensure Backspace remains `direction -1` with uncounted default offset `-1`.
   - Avoid changes to `.obsidian/hotkeys.json`, `.obsidian.vimrc`, task completion behavior, or unrelated vault files
     unless direct evidence points there.

5. If any files under `~/bob` are changed, commit only the task-related files using the required `/sase_git_commit`
   workflow before terminating. Do not stage or revert unrelated dirty vault files.

## Acceptance Criteria

- Bare normal-mode `<Enter>` reads links from the cursor's current line.
- Bare normal-mode `<Enter>` opens/creates a single actionable current-line link.
- Bare normal-mode `<Enter>` opens the existing link picker for multiple actionable current-line links.
- Bare normal-mode `<Enter>` with no actionable current-line link still falls through to one-line-down movement.
- `1<Enter>` targets one line below the cursor for link open/create.
- `5<Enter>` targets five lines below the cursor for link open/create.
- Bare Backspace link behavior still targets the previous line.
- Ctrl+Enter task toggle behavior is unchanged.
- Unrelated dirty vault files remain untouched.

## Verification Commands

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/task-status-cycler/main.js \
  .obsidian.vimrc \
  .obsidian/hotkeys.json
```

Focused VM helper check:

```bash
node - <<'NODE'
const Module = require('module');
const originalLoad = Module._load;
Module._load = function(request, parent, isMain) {
  if (request === 'obsidian') {
    class Modal {}
    class Plugin {}
    class MarkdownView {}
    class Notice {}
    return { MarkdownView, Modal, Notice, Plugin, setIcon: () => {} };
  }
  if (request === '@codemirror/view') {
    return { EditorView: { updateListener: { of: (fn) => fn } } };
  }
  return originalLoad.apply(this, arguments);
};
const nav = require('/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js');
const h = nav.helpers;
const cm = {
  getCursor: () => ({ line: 10, ch: 3 }),
  firstLine: () => 0,
  lastLine: () => 20,
  getLine: () => '',
};
console.log(JSON.stringify({
  bareEmptyArgs: h.getVimEnterTargetLine(cm, {}),
  bareNullArgs: h.getVimEnterTargetLine(cm, null),
  bareUndefinedRepeat: h.getVimEnterTargetLine(cm, { repeat: undefined }),
  explicitRepeat1: h.getVimEnterTargetLine(cm, { repeat: 1 }),
  explicitRepeat5: h.getVimEnterTargetLine(cm, { repeat: 5 }),
  backspaceBare: h.getVimBackspaceTargetLine(cm, {}),
}));
NODE
```
