---
create_time: 2026-06-05 16:25:32
status: wip
prompt: sdd/prompts/202606/enter_current_line.md
---
# Enter Current-Line Target Plan

## Goal

Fix Vim normal-mode `<enter>` in the Bob Obsidian vault so a bare Enter targets the current line for link open/create.
Explicit Vim counts should keep the relative-line behavior from the approved Enter link plan, so `5<enter>` still
targets five lines below the cursor.

## Context Reviewed

- Read the SASE plan skill instructions in `/home/bryan/.codex/skills/sase_plan/SKILL.md`.
- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault workflow constraints before planning a fix for Enter link-jump behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect status first, preserve unrelated pre-existing changes, and
  commit only task-related files with `/sase_git_commit` before terminating if any files under `~/bob` are changed.
- Read the approved Enter link plan in `sdd/tales/202606/enter_link_jump_create.md`.
- Inspected the current affected files:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- Inspected recent vault commits:
  - `d715955 feat: add Enter link jump creation`
  - `00ab1f8 feat: move vim task completion to Ctrl+Enter`

## Current Findings

- The approved plan intentionally defined line targeting as `cursor.line + repeat`, with default repeat `1`. That made
  bare Enter look one line below the cursor.
- The current navigation plugin still implements that exact rule: `getVimOffsetTargetLine(...)` normalizes a missing
  `actionArgs.repeat` to `1`, then computes `cursor.line + direction * repeat`.
- The current task plugin maps bare `<CR>` to link navigation plus fallback movement. Ctrl+Enter owns task completion
  after `00ab1f8`, so this fix should not restore task toggling to bare Enter.
- The vault currently has pre-existing unstaged changes in:
  - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `.obsidian/plugins/task-status-cycler/main.js`
  - `.obsidian/hotkeys.json`
  - `.obsidian/plugins/templater-obsidian/data.json`
- The unstaged plugin changes include Backspace previous-line link behavior and subpath-only link resolution. The Enter
  fix must preserve those changes and avoid a broad helper change that accidentally makes Backspace target the current
  line.

## Product Decisions

1. Bare Enter should target the current line for link open/create.
   - If the current line has one actionable internal note link, open/create it.
   - If the current line has multiple actionable links, show the existing filtered picker.
   - If the current line has no actionable links, preserve the existing fallthrough movement behavior.

2. Explicit counts should remain relative offsets.
   - `1<enter>` targets one line below the cursor.
   - `5<enter>` targets five lines below the cursor.
   - Counts are clamped to the editor bounds as before.

3. Fallback movement should remain separate from link targeting.
   - Bare Enter with no link should still move down one line, matching the current fallthrough behavior.
   - Counted Enter with no link should still move down by the normalized count.

4. The implementation should distinguish "no count was provided" from "normalize this count to 1".
   - Add a helper such as `hasVimRepeat(actionArgs)` or `getVimTargetOffset(actionArgs, direction, defaultOffset)`.
   - Use a current-line default offset only for Enter link targeting.
   - Preserve existing repeat normalization for explicit counts and fallback movement.

5. Preserve the current Backspace behavior in the dirty working tree.
   - If keeping the shared offset helper, pass a per-key default offset: Enter defaults to `0`, Backspace defaults to
     `-1`.
   - Do not change the task plugin's Backspace fallback semantics while fixing Enter.

## Implementation Scope

Expected vault files to edit after this plan is submitted:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js` only if tests show fallback/delegation needs a small
  companion change.

No expected edits:

- `.obsidian/hotkeys.json`
- `.obsidian/plugins/templater-obsidian/data.json`
- `_templates/new_note.md`
- `bob-cli` Rust source

Likely navigation plugin changes:

- Add a pure helper that detects whether a Vim repeat/count was explicitly supplied.
- Update Enter target-line calculation so missing repeat produces offset `0`.
- Keep explicit positive counts as `direction * repeat`.
- Keep invalid explicit counts safely normalized to `1`.
- Preserve Backspace/default previous-line behavior by passing an explicit uncounted default offset for that path.
- Export any new pure helper under `module.exports.helpers` for focused VM checks.

Likely task plugin changes:

- Prefer no task-plugin change if navigation can fully own the corrected target semantics.
- If needed, update tests or helper exports only; keep bare `<CR>` mapped to link navigation plus movement fallback and
  keep Ctrl+Enter as task toggle.

## Acceptance Criteria

- Bare `<enter>` on a current line with one existing wikilink opens that note.
- Bare `<enter>` on a current line with one missing safe wikilink creates it from `_templates/new_note.md` and opens it.
- Bare `<enter>` on a current line with multiple actionable links opens the existing filtered picker.
- Bare `<enter>` on a current line with no actionable link still falls through to the current movement behavior.
- `1<enter>` targets the line one below the cursor for link open/create.
- `5<enter>` targets the line five below the cursor for link open/create.
- Counted Enter fallback movement still moves by the normalized count.
- Existing Backspace previous-line link behavior in the current working tree is preserved.
- Existing Ctrl+Enter task toggle behavior is unchanged.
- No unrelated dirty vault files are modified, staged, reverted, or committed.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/task-status-cycler/main.js
```

Focused Node VM checks with stubbed Obsidian modules:

- `getVimEnterTargetLine(cm, {})` targets `cursor.line`.
- `getVimEnterTargetLine(cm, null)` targets `cursor.line`.
- `getVimEnterTargetLine(cm, { repeat: undefined })` targets `cursor.line`.
- `getVimEnterTargetLine(cm, { repeat: 1 })` targets `cursor.line + 1`.
- `getVimEnterTargetLine(cm, { repeat: 5 })` targets `cursor.line + 5`.
- Enter targeting clamps at first/last editor lines.
- Backspace no-count targeting, if present, still targets `cursor.line - 1`.
- `handleVimEnterLinkAction` reads links from the current line when no repeat is supplied.
- Task plugin bare Enter still delegates to navigation first and falls through only when navigation returns `false`.

Manual live-vault check after implementation and plugin reload if feasible:

- Place the cursor on a line containing a single `[[note]]`; press Enter and confirm it opens.
- Place the cursor on a line containing multiple links; press Enter and confirm the picker opens.
- Use `5<enter>` on a note where the line five below contains a single link and confirm it opens that target.

## Commit Plan

After implementation and verification, if any files under `~/bob` were changed, commit only the task-related changed
files with `/sase_git_commit`. Leave unrelated dirty vault files untouched.
