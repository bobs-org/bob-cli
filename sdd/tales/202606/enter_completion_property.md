---
create_time: 2026-06-04 18:33:09
status: wip
prompt: sdd/prompts/202606/enter_completion_property.md
---
# Plan: Obsidian Enter Task Completion Property

## Context

- This work targets the live Obsidian vault at `/home/bryan/bob`, not the `bob-cli` Rust code and not the Hammerspoon
  capture configuration.
- Project Obsidian memory was reviewed through:
  `sase memory read long/obsidian.md --reason "Need Bob/Obsidian task workflow context before planning Hammerspoon Enter completion-property behavior"`.
- The vault's local instructions in `/home/bryan/bob/AGENTS.md` require checking vault status before editing, preserving
  unrelated dirty files, and committing only task-related files changed for the current task before termination.
- `/home/bryan/bob` is already dirty in many unrelated notes and plugin/settings files. The relevant target file,
  `.obsidian/plugins/task-status-cycler/main.js`, is currently clean.
  `.obsidian/plugins/obsidian-tasks-plugin/data.json` is dirty only because of a missing trailing newline and should not
  be touched for this task.
- The bare Enter behavior is not in Hammerspoon or `.obsidian/hotkeys.json`. It is registered by
  `.obsidian/plugins/task-status-cycler/main.js` as a Vim normal-mode mapping:
  `vim.mapCommand("<CR>", "action", "taskStatusCyclerToggleOpenDone", {}, { context: "normal" })`.
- The current `<CR>` path calls `toggleActiveCheckboxOpenDone()`, which uses `setActiveCheckboxStatusLocal()` and only
  swaps the checkbox character between `[ ]` and `[x]`. That bypasses the existing `setActiveCheckboxStatus()` path,
  which already delegates `#task` status changes to Obsidian Tasks commands.
- The installed Tasks plugin is `obsidian-tasks-plugin` version `8.0.0`. Its settings use:
  - `globalFilter: "#task"`
  - `taskFormat: "dataview"`
  - `setDoneDate: true`
  - `setCreatedDate: true`
  - `setCancelledDate: true`
- For Bryan's vault, the appropriate done-date field is the Dataview inline field `[completion:: YYYY-MM-DD]`, matching
  existing vault tasks and prior task-property research.

## Goal

When pressing `<enter>` / `<CR>` in Obsidian Vim normal mode on an open Bob task line, completing the task should add a
completion property whose value is the local date on which the task was completed:

```markdown
- [ ] #task Call Pat +- [x] #task Call Pat [completion:: YYYY-MM-DD]
```

Reopening a completed task with `<CR>` should remove the completion property when Tasks plugin behavior is available, or
perform the same removal in the local fallback path. Existing non-task fallthrough navigation should remain unchanged.

## Non-Goals

- Do not modify Hammerspoon capture behavior.
- Do not modify `bob-cli` subcommands, options, or Rust code.
- Do not modify `.obsidian/hotkeys.json`.
- Do not modify Tasks plugin settings unless implementation proves the current settings are not actually loaded.
- Do not edit vault notes as part of implementation, except for manual smoke-test scratch content if explicitly
  approved.
- Do not modify memory files.
- Do not rework unrelated `task-status-cycler` mappings such as `o`, `<C-d>`, or `<C-u>`.

## Implementation Approach

1. Keep the code edit scoped to `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
   - Re-check `git -C /home/bryan/bob status --short` immediately before editing.
   - Confirm `.obsidian/plugins/task-status-cycler/main.js` is still clean or only contains changes from this task.

2. Route the `<CR>` open/done toggle through the Tasks-aware status setter.
   - Change `toggleActiveCheckboxOpenDone(editor, taskStatus)` so it computes the next symbol (`"x"` for open, `" "` for
     done) and calls `setActiveCheckboxStatus(editor, taskStatus, nextSymbol)` instead of
     `setActiveCheckboxStatusLocal()`.
   - This makes the Vim `<CR>` mapping use the same command path as the existing Alt-[ / Alt-] cycling commands.
   - For `#task` lines, `setActiveCheckboxStatus()` should first execute the Tasks command
     `obsidian-tasks-plugin:set-status-symbol-to-x` or `obsidian-tasks-plugin:set-status-symbol-to-space`, allowing
     Tasks to add or remove `[completion:: YYYY-MM-DD]` and preserve recurrence semantics.
   - For non-`#task` checklist lines, keep the current local checkbox-only fallback.

3. Add a small local metadata fallback for command-unavailable cases.
   - Keep Tasks as the preferred path whenever `lineMatchesTasksGlobalFilter(taskStatus.lineText)` and
     `tryExecuteTasksCommand(commandId)` succeed.
   - If the line contains `#task` but the Tasks command is unavailable or returns `false`, have the local fallback:
     - replace the checkbox symbol;
     - when completing, add or replace `[completion:: YYYY-MM-DD]` using the local date at toggle time;
     - when reopening, remove an existing `[completion:: YYYY-MM-DD]` field;
     - insert the completion field before a trailing Obsidian block id such as `^call-pat`;
     - avoid duplicate completion fields.
   - Use small pure helpers for local-date formatting and task-line metadata rewriting so behavior can be tested without
     an Obsidian runtime.

4. Preserve existing behaviors outside this toggle.
   - Non-task lines in `handleVimEnterToggle()` should continue to fall through to `vimEnterFallthrough(cm)`.
   - Task lines whose statuses are not open/done should still fall through rather than being rewritten.
   - Existing status cycling should continue to use Tasks commands for `#task` lines and local fallback otherwise.

## Validation

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused Node harness with a stubbed `obsidian` module:

- `<CR>` on `- [ ] #task Call Pat` attempts `obsidian-tasks-plugin:set-status-symbol-to-x` before local editing.
- If the Tasks command succeeds, the plugin does not perform a second local rewrite.
- If the Tasks command is unavailable, the fallback rewrites to `- [x] #task Call Pat  [completion:: 2026-06-04]` for a
  fixed local date.
- If a block id is present, the fallback writes `- [x] #task Call Pat  [completion:: 2026-06-04] ^call-pat`.
- If an old completion property is present while completing, the fallback replaces it rather than duplicating it.
- Reopening `- [x] #task Call Pat  [completion:: 2026-06-04]` removes the completion field.
- Non-`#task` checklist lines still only toggle the checkbox symbol.
- Non-task lines still use the existing `<CR>` fallthrough behavior.

Git hygiene checks:

```bash
git -C /home/bryan/bob status --short
git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js
git status --short
```

Manual live-vault smoke test after reloading Obsidian or the `task-status-cycler` plugin:

- On an open `#task` line in Vim normal mode, press `<enter>` and confirm the line becomes checked and receives
  `[completion:: <today>]`, where `<today>` is the local date of the keypress.
- Press `<enter>` again on that completed line and confirm it returns to open and the completion property is removed.
- Repeat on a task with a trailing block id and confirm the block id remains final.
- Confirm `<enter>` on a non-task line still moves to the next line as before.

## Commit Handling

If implementation changes any file under `/home/bryan/bob`, commit only the file changed for this task with the required
`sase_git_commit` workflow before finishing. The expected commit scope is:

```text
.obsidian/plugins/task-status-cycler/main.js
```

Do not stage or commit pre-existing dirty note files, Tasks settings newline drift, or unrelated plugin changes.

## Risks

- The best integration path depends on Obsidian Tasks command behavior. Mitigation: prefer the Tasks command path and
  add local fallback tests only for command-unavailable cases.
- Local fallback cannot fully reproduce Tasks recurrence behavior. Mitigation: only use fallback when the Tasks command
  is not available or declines to run.
- Date formatting must use local time, not UTC. Mitigation: use `Date#getFullYear()`, `getMonth()`, and `getDate()` in
  the helper and test with a fixed local `Date`.
- Because this is a live Obsidian plugin, static and harness checks cannot fully prove Vim runtime behavior. Mitigation:
  include a manual smoke test after plugin reload.
