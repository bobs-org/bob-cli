---
create_time: 2026-06-19 07:52:49
status: wip
prompt: sdd/prompts/202606/conditional_pomodoro_creation.md
---
# Conditional Pomodoro Creation Plan

## Goal

Change the Obsidian Vim normal-mode `<Ctrl+Enter>` Pomodoro completion flow so it does not always insert a new
placeholder Pomodoro after completing the current one.

After the change, completing an open Pomodoro task creates a new `- [ ] ()` placeholder only when:

1. the current Pomodoro has one or more copyable bullets that will be carried forward under the new placeholder; or
2. the current Pomodoro is the last Pomodoro in the file's `## Pomodoros` section.

If neither condition is true, `<Ctrl+Enter>` should only complete the current Pomodoro, preserve the existing next
Pomodoro, and not add an empty placeholder between them.

## Context Reviewed

- Used the required `sase_plan` skill for this planning task.
- Read `memory/short/sase.md`: this is an ephemeral `bob-cli_<N>` workspace, so commands should stay scoped and avoid
  confusing it with the live vault.
- Because this is Obsidian behavior, read long-term memory through the audited command:
  `sase memory read obsidian.md --reason "Need Obsidian vault and keymap workflow constraints before planning Ctrl+Enter Pomodoro behavior"`.
- Read `/home/bryan/bob/AGENTS.md`: the live Obsidian vault may already be dirty; inspect status before editing; do not
  overwrite, stage, revert, or commit unrelated pre-existing changes; after any file changes under `~/bob`, commit only
  this task's changes using the SASE commit workflow.
- Inspected the live implementation in `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- Reviewed the prior related plans:
  - `sdd/tales/202606/pomodoro_ctrl_enter_close_and_create.md`
  - `sdd/tales/202606/pomodoro_next_cursor_jump.md`
  - `sdd/tales/202606/always_create_pomodoro_task.md`
- Checked vault status before planning. The target plugin file is currently clean; unrelated notes in `~/bob` are dirty
  and should be left alone.

## Current Behavior

The live code path is:

- `<C-CR>` and `<C-Enter>` are mapped to `taskStatusCyclerToggleTaskOpenDone`.
- `handleVimTaskToggleOpenDone()` detects an open Pomodoro task and routes to `completeActivePomodoroTask()`.
- `completeActivePomodoroTask()` completes any transcluded source tasks under the Pomodoro, recomputes the active editor
  buffer, builds a local edit plan with `buildPomodoroCompletionPlan()`, then applies it with
  `applyPomodoroCompletionPlan()`.
- `buildPomodoroCompletionPlan()` now unconditionally:
  - marks the current Pomodoro `[ ]` -> `[x]`;
  - sets `createdPomodoro = true`;
  - inserts `- [ ] ()` at `sourceRange.endLine`;
  - copies every `sourceBullets.copyableTaskLinkBullets` line under that new placeholder;
  - inserts one empty tab-indented sub-bullet when there are no copied lines;
  - sets `cursorTargetLine` to the inserted placeholder.

That unconditional placeholder insertion is the behavior to change.

## Product Decisions

1. Keep the behavior scoped to completing an open top-level Pomodoro task inside the `## Pomodoros` section.
2. Reopening a done Pomodoro remains a plain toggle with no sub-bullet completion, no copying, and no placeholder
   insertion.
3. "Bullets that will be copied" means the existing copyable bullet classification:
   `sourceBullets.copyableTaskLinkBullets`, which includes non-transcluded block-link task bullets and excludes
   transcluded bullets, note-only bullets, and non-block note links.
4. "Last one in the file" should use the existing Pomodoro-section model: no `findNextPomodoroLine(...)` result within
   the current `## Pomodoros` section.
5. When copyable bullets exist, create a new placeholder immediately after the completed Pomodoro's own sub-bullet
   block, even if another Pomodoro already exists below. Copy the bullets under the newly inserted placeholder; do not
   append them to the pre-existing next Pomodoro.
6. When the current Pomodoro is last, create the placeholder even with zero copyable bullets. Preserve the current empty
   tab-indented `- ` child line in that no-copy last-Pomodoro case.
7. When there is a next Pomodoro and zero copyable bullets, do not insert anything. The edit plan should contain only
   the status replacement for the completed Pomodoro.
8. Cursor behavior should remain useful:
   - if a placeholder is created, jump to it and place the cursor between the parens as today;
   - if no placeholder is created but an existing next Pomodoro exists, jump to that existing next Pomodoro line, using
     the existing placeholder-aware column helper in case that next Pomodoro is itself `- [ ] ()`.
9. Existing lower Pomodoros should never receive copied bullets as part of this change. They are either pushed down by a
   new placeholder or left untouched.

## Implementation Scope

Expected file to edit after plan approval:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No expected edits:

- `bob-cli` Rust, script, README, test, or memory files
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/obsidian_vimrc.md`
- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- vault notes, except optional scratch-note manual smoke testing that must be cleaned up before finishing

## Implementation Approach

1. Update `buildPomodoroCompletionPlan(lines, section, pomodoroLine)`:
   - keep the existing open Pomodoro validation;
   - keep computing `sourceRange`, `sourceBullets`, and `nextPomodoroLine`;
   - compute `copyableBulletLines` from `sourceBullets.copyableTaskLinkBullets.map((bullet) => bullet.lineText)`;
   - compute `isLastPomodoro = nextPomodoroLine === null`;
   - compute `shouldCreatePomodoro = copyableBulletLines.length > 0 || isLastPomodoro`.
2. If `shouldCreatePomodoro` is true:
   - set `createdPomodoro = true`;
   - push the existing `insertLines` edit at `sourceRange.endLine`;
   - insert `POMODORO_PLACEHOLDER_LINE`;
   - insert copied bullet lines when present, otherwise `EMPTY_POMODORO_SUB_BULLET_LINE`;
   - set `cursorTargetLine = sourceRange.endLine`.
3. If `shouldCreatePomodoro` is false:
   - set `createdPomodoro = false`;
   - leave `copiedBulletLines = []`;
   - do not push an `insertLines` edit;
   - set `cursorTargetLine = nextPomodoroLine`.
4. Keep `findNextPomodoroLine()`, `getPomodoroSubBulletTargetKeys()`, and `getNonDuplicateCopyablePomodoroBullets()` in
   place unless cleanup proves necessary. They are exported helper surface or low-risk unused code, and this change
   should stay tightly scoped.
5. Review `applyPomodoroCompletionPlan()` comments after changing the plan shape. Its existing code can already jump to
   `plan.cursorTargetLine`; only any stale comment that implies all cursor targets are newly created placeholders should
   be clarified.
6. Do not change Vim keymap registration or command IDs.

## Acceptance Criteria

Given a completed Pomodoro with a following Pomodoro and one copyable bullet:

```md
## Pomodoros

- [ ] (**0900-0925** [t:: 25m])
  - [[bob#^task-a]]
- [ ] (**0930-0955** [t:: 25m])
```

`<Ctrl+Enter>` on the first Pomodoro produces:

```md
## Pomodoros

- [x] (**0900-0925** [t:: 25m])
  - [[bob#^task-a]]
- [ ] ()
  - [[bob#^task-a]]
- [ ] (**0930-0955** [t:: 25m])
```

Given a completed Pomodoro with a following Pomodoro and no copyable bullets:

```md
## Pomodoros

- [ ] (**0900-0925** [t:: 25m])
  - [[gtd_daily]]
- [ ] (**0930-0955** [t:: 25m])
```

`<Ctrl+Enter>` on the first Pomodoro produces no inserted placeholder:

```md
## Pomodoros

- [x] (**0900-0925** [t:: 25m])
  - [[gtd_daily]]
- [ ] (**0930-0955** [t:: 25m])
```

Additional expected behavior:

- If the completed Pomodoro is the last Pomodoro, a new placeholder is still created.
- If the last Pomodoro has no copyable bullets, the new placeholder still gets one empty tab-indented `- ` sub-bullet.
- Transcluded task bullets are still completed in their source files before the local plan is built, but they are not
  counted as copyable bullets for placeholder creation.
- Note-only bullets and ordinary note links do not cause placeholder creation when a later Pomodoro already exists.
- Existing lower Pomodoros are never modified with appended copied bullets.
- The cursor lands on the newly created placeholder when one is created, otherwise on the pre-existing next Pomodoro.
- Non-Pomodoro task toggles, transcluded sub-bullet toggles, done-Pomodoro reopen toggles, and files without a
  `## Pomodoros` section keep their current behavior.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused helper checks using the exported `module.exports.helpers` surface with a stubbed `obsidian` module:

- `buildPomodoroCompletionPlan()` with copyable bullets and an existing next Pomodoro returns an `insertLines` edit at
  `sourceRange.endLine`, `createdPomodoro === true`, and `cursorTargetLine === sourceRange.endLine`.
- `buildPomodoroCompletionPlan()` with no copyable bullets and an existing next Pomodoro returns no `insertLines` edit,
  `createdPomodoro === false`, `copiedBulletLines` empty, and `cursorTargetLine === nextPomodoroLine`.
- `buildPomodoroCompletionPlan()` for the last Pomodoro creates the placeholder with copied bullets when present.
- `buildPomodoroCompletionPlan()` for the last Pomodoro with no copyable bullets creates the placeholder with exactly
  `EMPTY_POMODORO_SUB_BULLET_LINE`.
- `applyPomodoroCompletionPlan()` preserves cursor placement for created placeholders and existing next Pomodoros.

Optional manual smoke test after reloading the plugin in Obsidian:

- Complete a Pomodoro with copyable bullets and another Pomodoro below; confirm the new placeholder appears between them
  with copied bullets.
- Complete a Pomodoro with only note bullets and another Pomodoro below; confirm no placeholder appears.
- Complete the last Pomodoro with no copyable bullets; confirm the placeholder plus empty child bullet appears.
- Reopen a done Pomodoro; confirm no placeholder appears.

Vault hygiene before finishing implementation:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js
```

If implementation is approved and changes are made under `~/bob`, commit only this task's changes with the
`/sase_git_commit` workflow, leaving unrelated dirty vault files untouched.

## Risks

- The old pre-"always create" branch appended copied bullets to the existing next Pomodoro. This plan deliberately does
  not restore that behavior; when no new placeholder is created, no bullets are copied because there are no bullets that
  need carrying forward.
- Cursor behavior has to cover both created and non-created plans. The existing `applyPomodoroCompletionPlan()` already
  supports arbitrary `cursorTargetLine`, so this should be a small plan-builder change plus comment cleanup.
- The phrase "last one in the file" could be read literally across the entire file, but the plugin's Pomodoro model is
  section-scoped. Using `findNextPomodoroLine()` within `## Pomodoros` preserves established behavior and avoids
  treating unrelated later tasks as Pomodoros.
