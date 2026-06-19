---
create_time: 2026-06-19 10:03:32
status: done
prompt: sdd/prompts/202606/ctrl_shift_jk_pomodoro_tasks.md
---
# Plan: Include Pomodoro Tasks in Ctrl+Shift+J/K Navigation

## Goal

Update the existing Bob vault `<Ctrl+Shift+J>` and `<Ctrl+Shift+K>` open-task navigation so it cycles through both:

- proper open Obsidian task lines, as it does today: checkbox list items with standalone `#task` and open statuses
  `[ ]`, `[/]`, or `[B]`;
- open Pomodoro ledger task lines in daily-note `## Pomodoros` sections, even when they do not carry `#task`.

The keymaps, command ids, circular wrap behavior, centered post-jump scroll, and duplicate-dispatch guard should remain
unchanged. Only the set of recognized navigation targets should broaden.

## Context Reviewed

- Required Obsidian long-term memory was read through:
  `sase memory read obsidian.md --reason "Need Obsidian vault task conventions before planning daily vault pomodoro task keymap support"`.
- This is live Obsidian vault work under `/home/bryan/bob`, not Rust `bob-cli` work. No CLI subcommands or options are
  planned, so `memory/long/cli_rules.md` does not apply.
- `/home/bryan/bob/AGENTS.md` requires checking vault status before edits, preserving unrelated dirty files, and
  committing task-related vault edits with `/sase_git_commit` before terminating after implementation.
- Current relevant vault status shows a pre-existing `obsidian_vimrc.md` edit and no current
  `.obsidian/plugins/bob-navigation-hotkeys/main.js` diff. Any later implementation must preserve the vimrc change and
  avoid treating it as part of this task.
- The current `<Ctrl+Shift+J/K>` implementation lives in
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- The existing implementation already provides:
  - `isOpenObsidianTaskLine(...)` for proper `#task` checkbox lines;
  - `getOpenObsidianTaskLines(...)`, which skips frontmatter and fenced code blocks;
  - circular `getOpenObsidianTaskJumpLine(...)`;
  - `jumpToOpenObsidianTask(...)`, including same-dispatch dedupe and Vim `zz`-style centered scrolling;
  - capture-phase Ctrl+Shift+J/K fallback for Vim normal mode.
- Current daily notes and `_templates/daily.md` use `## Pomodoros` sections with top-level checkbox entries like
  `- [ ] ()` and `- [ ] (**2050-2125** [t:: 35m])`, often without `#task`. These are intentionally missed by the current
  proper-task-only matcher.
- Existing Pomodoro logic in sibling vault plugins treats Pomodoros as top-level task lines inside `## Pomodoros`;
  `bob-ledger-tools` considers checkbox statuses `[ ]` and `[/]` open, and recognizes either a time range or `()`
  placeholder.

## Product Decisions

1. Keep "proper Obsidian task" matching exactly as-is. Do not change the `#task` boundary regex, open status set, or
   frontmatter/fence exclusions for ordinary task navigation.

2. Add Pomodoro navigation targets only for top-level checkbox lines inside a `## Pomodoros` section. This avoids
   treating carried-forward indented bullets under a Pomodoro as separate targets.

3. A Pomodoro target should be open and ledger-shaped:
   - checkbox status is `[ ]` or `[/]`;
   - the line contains either a compact/colon time range in parentheses or an empty `()` placeholder.

4. Done and canceled Pomodoros are not navigation targets. That excludes `[x]`, `[X]`, and `[-]`.

5. Do not broaden navigation to arbitrary checkboxes without `#task`. Outside `## Pomodoros`, plain checklists remain
   ignored.

6. Use the current circular list behavior after merging targets:
   - next moves to the nearest lower target, wrapping to the first target when needed;
   - previous moves to the nearest higher target, wrapping to the last target when needed;
   - a no-target notice appears only when there are no targets, or the sole target is already selected.

7. If a line qualifies as both a proper `#task` and a Pomodoro line, it appears only once in the navigation target list.

8. Keep all binding surfaces stable:
   - no `.obsidian/hotkeys.json` changes;
   - no `obsidian_vimrc.md` changes;
   - no command id changes;
   - no changes to the Ctrl+Shift+J/K physical-key fallback.

## Implementation Approach

Edit only `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

### 1. Add Pomodoro section and ledger helpers

Add small local helpers near the existing open-task scanner:

- `POMODOROS_HEADING_RE = /^##\s+Pomodoros(?:\s.*)?$/`
- `LEVEL_TWO_HEADING_RE = /^##\s+/`
- `OPEN_POMODORO_STATUSES = new Set([" ", "/"])`
- a top-level ledger checkbox regex aligned with existing vault Pomodoro behavior;
- compact and colon time-range regexes, or a small shared `hasPomodoroTimeRange(...)` helper;
- `isOpenPomodoroTaskLine(lineText)`, true only when the line is top-level, open, and has a time range or `()`;
- `isPomodorosHeading(lineText)` / section-state helper if useful.

Keep these helpers local to `bob-navigation-hotkeys`; do not import or edit `bob-ledger-tools` or `task-status-cycler`.
The duplication is small and avoids coupling independent Obsidian plugins through non-public module internals.

### 2. Scan one combined target list

Preserve `isOpenObsidianTaskLine(...)` for proper task checks.

Replace the current proper-task-only target collection with a scanner that walks the current file once and tracks:

- leading frontmatter state;
- fenced-code state using the existing `getFenceOpening(...)` and `isClosingFence(...)` helpers;
- whether the current line is inside a real Markdown `## Pomodoros` section.

For each normal content line:

1. Update Pomodoros section state on unfenced level-two headings.
2. Add the line index if `isOpenObsidianTaskLine(line)` is true.
3. Otherwise add the line index if currently inside `## Pomodoros` and `isOpenPomodoroTaskLine(line)` is true.

Name options:

- Prefer adding `getOpenTaskNavigationLines(lines)` for the combined list and leaving `getOpenObsidianTaskLines(lines)`
  available for existing proper-task helper tests.
- Then update `getOpenObsidianTaskJumpLine(...)` to use the combined navigation list while preserving its public name
  and command behavior.

This keeps external command ids stable while making the broadened target set explicit in the code.

### 3. Keep jump execution unchanged

Do not change:

- `jumpToOpenObsidianTask(...)` notices;
- same-dispatch deduplication;
- `scheduleOpenTaskJumpCenter(...)`;
- section-header navigation;
- dash-task navigation;
- project/task promotion helpers.

Because `getOpenObsidianTaskJumpLine(...)` already receives a target list and performs circular selection, the intended
behavior should come from changing target collection only.

## Validation Plan

Run static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Run focused throwaway Node tests with stubbed `obsidian` and `@codemirror/view`, then remove the temporary files.

Cover parser and target-list cases:

- Existing proper-task truths still pass: `- [ ] #task`, `- [/] #task`, `- [B] #task`, ordered/indented/blockquote
  proper tasks.
- Existing proper-task false cases still pass: done/canceled tasks, missing `#task`, `#taskish`, prose, frontmatter
  examples, fenced examples.
- Pomodoro target truths:
  - `- [ ] ()` under `## Pomodoros`;
  - `- [ ] (**2050-2125** [t:: 35m])` under `## Pomodoros`;
  - `- [/] (20:50-21:25)` under `## Pomodoros`, if supported by existing ledger tools.
- Pomodoro false cases:
  - identical lines outside `## Pomodoros`;
  - indented carried-forward bullets under a Pomodoro;
  - `[x]`, `[X]`, and `[-]` Pomodoros;
  - top-level open checkbox under `## Pomodoros` with neither time range nor `()`;
  - task-looking lines in fenced blocks inside a daily note.
- Combined ordering: proper tasks and Pomodoro tasks are returned in file order with no duplicate for a Pomodoro that
  also contains `#task`.

Cover circular jump behavior:

- From a normal `#task`, next can land on a later open Pomodoro.
- From an open Pomodoro, next can land on a later normal `#task`.
- From the last target, next wraps to the first target, regardless of target kind.
- From the first target, previous wraps to the last target, regardless of target kind.
- A single open Pomodoro on the current cursor line returns `null` and produces the existing no-target notice path.

Cover command behavior:

- `jumpToOpenObsidianTask(...)` still sets `{ line: targetLine, ch: 0 }`, schedules centered scrolling on success, and
  leaves notices unchanged.
- Duplicate synchronous dispatch still moves or notifies only once.

Manual Obsidian smoke test after reloading or toggling `bob-navigation-hotkeys`:

1. In `/home/bryan/bob/2026/20260619.md` or a scratch daily-format note, place the cursor before open placeholder
   Pomodoros such as `- [ ] ()`; `<Ctrl+Shift+J>` should land on the Pomodoro line and center it.
2. From that Pomodoro line, `<Ctrl+Shift+K>` should move to the previous navigation target and wrap as expected.
3. Confirm completed Pomodoros are skipped.
4. Confirm indented carried-forward bullets below Pomodoros are skipped.
5. Confirm ordinary non-`#task` checkboxes outside `## Pomodoros` are still skipped.
6. Confirm `<Ctrl+J>` and `<Ctrl+K>` still navigate section headers.

Review final diff and vault status:

```bash
git -C /home/bryan/bob diff -- .obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob status --short
```

If implementation proceeds later, stage and commit only
`/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` through `/sase_git_commit`, leaving the pre-existing
`obsidian_vimrc.md` change and unrelated vault files untouched.

## Risks And Mitigations

- **Accidentally navigating every checkbox:** require both `## Pomodoros` section context and ledger shape for
  non-`#task` Pomodoro targets.
- **Jumping to Pomodoro sub-bullets:** require top-level Pomodoro checkbox lines.
- **Breaking prior proper-task navigation:** leave `isOpenObsidianTaskLine(...)` unchanged and add regression checks for
  its current true/false cases.
- **False positives in examples:** keep the existing frontmatter/fence state machine in the combined scanner.
- **Plugin coupling:** copy the minimal Pomodoro parser into `bob-navigation-hotkeys` instead of importing internals
  from other local plugins.
- **Dirty vault state:** inspect status before editing and commit only the implementation file if this plan is approved
  for execution.
