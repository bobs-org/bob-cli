---
create_time: 2026-06-05 15:28:37
status: done
prompt: sdd/prompts/202606/ctrl_enter_task_completion.md
---
# Plan: Move Obsidian Task Completion From Enter to Ctrl+Enter

## Goal

Separate the two behaviors currently sharing the Vim normal-mode Enter keymap in the Bob Obsidian vault:

- `<Enter>` / `<CR>` should be the link-jump/create keymap only.
- `<Ctrl+Enter>` should be the explicit task open/done completion keymap.

The task completion behavior must preserve the existing Tasks-aware path, including adding/removing
`[completion:: YYYY-MM-DD]` for `#task` lines through the Tasks plugin when available and through the local fallback
when needed.

## Context Reviewed

- Read `memory/short/sase.md`.
- Read Obsidian long-term memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian workflow context before planning task-completion keymap migration"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits require checking status first, preserving unrelated dirty changes, and
  committing only current-task vault edits with the SASE git commit workflow after implementation.
- Inspected current and prior plans:
  - `sdd/tales/202606/enter_completion_property.md`
  - `sdd/tales/202606/enter_link_jump_create.md`
  - `sdd/tales/202606/enter_link_all_link_types.md`
  - `sdd/tales/202606/vim_backspace_link_action.md`
- Inspected live vault files:
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/hotkeys.json`
  - `/home/bryan/bob/.obsidian.vimrc`
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`

No `bob-cli` Rust CLI subcommands or options are involved, so `memory/long/cli_rules.md` is not required.

## Current State

- The live keymap owner is `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- `registerVimMappings()` currently defines `taskStatusCyclerToggleOpenDone` and maps normal-mode `<CR>` to it.
- `handleVimEnterToggle(cm, actionArgs)` currently:
  - toggles the current line first when repeat is `1` and the active line is an open/done task;
  - otherwise delegates to `bob-navigation-hotkeys.handleVimEnterLinkAction(cm, actionArgs)`;
  - falls through to repeat-aware downward line movement when no link action handles the key.
- The task toggle path now uses `toggleActiveCheckboxOpenDone()` -> `setActiveCheckboxStatus()`, which first tries the
  Tasks plugin command and falls back to local metadata rewriting for `#task` lines. That is the behavior to move.
- `bob-navigation-hotkeys` already owns the link behavior behind Enter, including counted line offsets, open/create,
  same-file heading/block links, and the multi-link picker.
- `.obsidian/hotkeys.json` currently has no `toggle-task-open-done` binding and is already modified before this task.
- The target vault currently has pre-existing dirty files:
  - `.obsidian/hotkeys.json`
  - `.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `.obsidian/plugins/task-status-cycler/main.js` These must be preserved and worked with, not reverted. The existing
    plugin diffs appear to come from recent Enter-link and Backspace-link work.

## Product Decisions

1. **Bare Enter becomes link-only.**
   - `<CR>` should no longer inspect or toggle the active task line.
   - It should call the navigation plugin's Enter link action first, then keep the existing repeat-aware downward
     fallthrough when no actionable link is found.
   - Existing counted Enter behavior remains unchanged: `N<Enter>` targets `cursor.line + N`.

2. **Ctrl+Enter becomes task-only.**
   - `<Ctrl+Enter>` should toggle only the active line when it is an open/done checklist item.
   - It should use the same `toggleActiveCheckboxOpenDone()` path so `#task` completion metadata remains correct.
   - On a non-task line or on non-open/done statuses such as `/`, `B`, or `-`, it should no-op without delegating to
     link jumping or moving the cursor. This keeps the two features genuinely separate.
   - Vim counts are not meaningful for task completion; ignore any repeat passed with Ctrl+Enter.

3. **Keep registration in `task-status-cycler`.**
   - This plugin already owns the CodeMirror Vim registration lifecycle and task commands.
   - Avoid `.obsidian.vimrc`; exmap/obcommand does not preserve the current CodeMirror Vim count semantics.
   - Avoid `.obsidian/hotkeys.json` unless implementation proves CodeMirror Vim cannot bind Ctrl+Enter reliably, because
     that file is already dirty and the existing behavior is a Vim mapping.

4. **Verify the Ctrl+Enter Vim token during implementation.**
   - The likely CodeMirror Vim mapping token is either `<C-CR>` or `<C-Enter>`.
   - Implementation should validate the accepted token with a focused registration test and, if possible, a live
     Obsidian smoke check.
   - If both forms are accepted and harmless, mapping both to the same action is acceptable; otherwise use the single
     token proven to work in this Obsidian environment.

## Implementation Scope

Expected live-vault file to edit:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No planned changes to:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian.vimrc`
- `bob-cli` Rust source files
- memory files

### `task-status-cycler/main.js`

- Replace the `<CR>` mapping target with a link-only action, for example:
  - define `taskStatusCyclerOpenNextLineLink` -> `handleVimEnterLinkOrFallthrough(cm, actionArgs)`;
  - map `<CR>` to that action in normal mode.
- Add a Ctrl+Enter task action, for example:
  - define `taskStatusCyclerToggleTaskOpenDone` -> `handleVimTaskToggleOpenDone(cm)`;
  - map the verified Ctrl+Enter token to that action in normal mode.
- Keep the existing Obsidian command `toggle-task-open-done`; it already uses `handleToggleOpenDoneCommand()` and can
  stay available for command palette or future hotkey binding.
- Refactor `handleVimEnterToggle()` to avoid misleading naming:
  - either rename it to `handleVimTaskToggleOpenDone()` and update tests/exports if present;
  - or keep a wrapper for compatibility while moving `<CR>` away from it.
- Add `handleVimEnterLinkOrFallthrough(cm, actionArgs)`:
  - compute `repeat` with existing `getVimRepeat(actionArgs)`;
  - call `handleVimEnterLinkAction(cm, actionArgs)`;
  - if not handled, call `vimEnterFallthrough(cm, repeat)`.
- Add `handleVimTaskToggleOpenDone()`:
  - get the active `MarkdownView`;
  - get the active task status from `view.editor`;
  - if it is open/done, call `toggleActiveCheckboxOpenDone(view.editor, taskStatus)`;
  - otherwise return without link delegation or fallthrough.
- Preserve existing Backspace, `o`, `<C-d>`, `<C-u>`, Alt-[, and Alt-] behavior.
- Preserve the existing task metadata helpers and Tasks plugin command fallback unchanged unless tests expose a bug.

## Acceptance Criteria

- Pressing `<Enter>` on an open or done task no longer toggles that task.
- Pressing `<Enter>` still opens/creates an actionable link on the target line through `bob-navigation-hotkeys`.
- Pressing `N<Enter>` still targets `cursor.line + N`.
- Pressing `<Enter>` with no actionable link still moves down to the computed target line's first nonblank character.
- Pressing `<Ctrl+Enter>` on `- [ ] #task Call Pat` marks it done and adds/removes completion metadata using the same
  behavior as the current Enter task toggle.
- Pressing `<Ctrl+Enter>` again on the completed task reopens it and removes `[completion:: YYYY-MM-DD]`.
- Pressing `<Ctrl+Enter>` on a non-`#task` checklist toggles only the checkbox symbol, as the current fallback does.
- Pressing `<Ctrl+Enter>` on a non-task line does not jump links and does not move the cursor.
- Backspace link action and all existing task status cycle commands still work.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused Node VM checks with stubbed `obsidian`, fake CodeMirror Vim, fake editor, and fake plugin registry:

- Vim registration maps `<CR>` to the link/fallthrough action, not the task-toggle action.
- Vim registration maps the proven Ctrl+Enter token to the task-toggle action in normal mode.
- If both `<C-CR>` and `<C-Enter>` are deliberately registered, both point at the same task-toggle action and no other
  mappings change.
- Bare Enter on an active open/done task delegates to `handleVimEnterLinkAction` instead of calling
  `toggleActiveCheckboxOpenDone`.
- Bare Enter falls through with the existing repeat count when the navigation plugin returns false, is unavailable, or
  throws.
- Ctrl+Enter on an active open/done task calls `toggleActiveCheckboxOpenDone`.
- Ctrl+Enter on a non-task line does not call link delegation and does not call `vimEnterFallthrough`.
- Ctrl+Enter uses the existing `setActiveCheckboxStatus()` path, so Tasks command success prevents duplicate local
  rewrites and Tasks command failure uses the completion-property fallback.
- Existing Backspace delegation/fallthrough tests still pass.

Manual live-vault smoke test after plugin reload:

1. In a scratch note, put the cursor on an open `#task` line and press Enter; confirm the task does not toggle.
2. Put an actionable link on the Enter target line and press Enter; confirm the link opens or creates as before.
3. Press Ctrl+Enter on the open `#task`; confirm it becomes done and gets `[completion:: <today>]`.
4. Press Ctrl+Enter again; confirm it reopens and removes the completion property.
5. Press Ctrl+Enter on a plain non-task line with links nearby; confirm no link jump and no cursor movement.
6. Confirm Backspace link navigation still works.

Git hygiene checks before implementation finish:

```bash
git -C /home/bryan/bob status --short -- \
  .obsidian/plugins/task-status-cycler/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/hotkeys.json \
  .obsidian.vimrc
git status --short
```

If implementation changes files under `/home/bryan/bob`, commit only the current task's vault changes with the
`sase_git_commit` workflow before finishing. Because the target file is already dirty before this task, inspect the
pre-change diff and final diff carefully so the commit does not stage unrelated prior work unless the user explicitly
approves bundling it.

## Risks

- **Ctrl+Enter token mismatch.** CodeMirror Vim may accept `<C-CR>`, `<C-Enter>`, or only one of them. Mitigation:
  verify registration in a focused test and live smoke test; use both only if proven safe.
- **Uncommitted vault changes.** The target plugin is already dirty. Mitigation: capture the pre-change diff, edit only
  the keymap/handler area, and avoid reverting or staging unrelated hunks.
- **Enter task regression intentionality.** Bare Enter no longer completes tasks by design. Mitigation: preserve the
  same task path on Ctrl+Enter and document manual smoke checks clearly.
- **Tasks plugin behavior.** Completion metadata depends on existing Tasks command behavior for `#task` lines.
  Mitigation: keep the current `setActiveCheckboxStatus()` implementation unchanged and keep local fallback coverage.
