---
create_time: 2026-06-19 08:08:54
status: proposed
prompt: sdd/prompts/202606/open_task_notice_dedup.md
---

# Plan: Deduplicate Ctrl+Shift+J/K Open-Task Notices

## Goal

Fix the Bob vault open-task navigation so a single `<Ctrl+Shift+J>` or `<Ctrl+Shift+K>` press shows at most one
`No next open task` or `No previous open task` notice.

The existing circular task selection behavior should remain unchanged:

- with multiple open `#task` lines, next/previous still cycles across file boundaries;
- with one open `#task` line and the cursor elsewhere, either direction still jumps to that task;
- with no open `#task` lines, or with the only open task already on the cursor line, the command still shows the
  existing direction-specific notice;
- section-header navigation remains strict and unchanged.

## Context Reviewed

- Required Obsidian long-term memory was read through:
  `sase memory read obsidian.md --reason "Need Obsidian workflow context before planning a fix for duplicate open-task navigation notices"`.
- This changes the live Obsidian vault at `/home/bryan/bob`, not the Rust `bob-cli` implementation. No new CLI
  subcommands or options are planned, so `memory/long/cli_rules.md` does not apply.
- `/home/bryan/bob` currently has unrelated dirty notes, `obsidian_vimrc.md`, and
  `.obsidian/plugins/task-status-cycler/main.js`. The target plugin file is clean.
- The relevant implementation is `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- The previous cycling change only edited `getOpenObsidianTaskJumpLine()`. The duplicate notice is not caused by two
  notice calls in that helper; the single notice branch is in `jumpToOpenObsidianTask(editor, direction)`.
- There are two paths that can invoke the same open-task jump method for the same physical chord:
  - the registered Obsidian editor commands bound in `.obsidian/hotkeys.json`;
  - the capture-phase Vim-normal fallback in `registerOpenTaskJumpInputListeners()` /
    `handleOpenTaskJumpPhysicalKeydown()`.
- The current `WeakSet` only prevents duplicate handling between the plugin's own `window` and `document` capture
  listeners. It does not deduplicate the Obsidian command path against the plugin fallback path.

## Working Diagnosis

The duplicate notice is most likely a double dispatch of one physical `<Ctrl+Shift+J/K>` keydown through both the
Obsidian hotkey command and the plugin's Vim-normal fallback. When the command has no movement target, both invocations
reach `jumpToOpenObsidianTask()` with the same editor and direction, and both construct the same `Notice`.

This should be fixed at the open-task command dispatch layer, not by changing task parsing or by hiding notices
globally. The guard should also prevent a possible double movement if both dispatch paths ever fire for a successful
jump.

## Product Decisions

1. Keep the notice text exactly as-is: `No next open task` and `No previous open task`.

2. Keep `.obsidian/hotkeys.json` unchanged. The normal Obsidian binding is still needed for insert mode and non-Vim
   editing.

3. Keep the Vim-normal capture fallback. It is still needed because CodeMirror Vim can swallow `<Ctrl+Shift+J/K>` before
   Obsidian's hotkey dispatcher handles it.

4. Deduplicate only same-dispatch duplicate open-task jumps. Do not introduce a long debounce that would make deliberate
   repeated presses or key repeat feel unresponsive.

5. Do not change `getOpenObsidianTaskJumpLine()` unless a focused regression test exposes a real bug in circular target
   selection.

## Implementation Approach

### 1. Add a short same-dispatch guard for open-task jumps

Edit `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

Add a small instance-level guard used by all open-task jump paths:

- key the guard by editor object plus direction;
- when `jumpToOpenObsidianTask(editor, direction)` starts, suppress the call if the same editor/direction is already
  marked for the current JavaScript dispatch turn;
- otherwise mark it before doing any cursor movement or notice creation;
- clear the mark on the next macrotask, such as with `setTimeout(..., 0)`, so an intentional later keypress still works;
- handle missing or invalid editor values without throwing.

This keeps deduplication local to the command that is affected. It avoids a time-window debounce that could swallow fast
intentional repeated navigation.

### 2. Keep all keybinding and task-selection surfaces stable

Do not change:

- `.obsidian/hotkeys.json`;
- `obsidian_vimrc.md`;
- command ids `jump-to-next-open-task` and `jump-to-prev-open-task`;
- task regexes and `getOpenObsidianTaskLines()`;
- circular selection semantics in `getOpenObsidianTaskJumpLine()`;
- section-header navigation.

If implementation review shows that command registration should call a tiny wrapper instead of
`jumpToOpenObsidianTask()` directly, keep the wrapper private and preserve the public command ids.

### 3. Add focused regression coverage with throwaway Node tests

Use the same temporary Node harness strategy as the previous task:

- stub `obsidian` and `@codemirror/view`;
- load `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`;
- instantiate the plugin class with a minimal app/view/editor shape;
- remove all temporary files after the run.

Cover duplicate-dispatch behavior:

- calling `jumpToOpenObsidianTask(editor, 1)` twice synchronously in a zero-task note emits exactly one
  `No next open task` notice;
- calling `jumpToOpenObsidianTask(editor, -1)` twice synchronously while the sole task is on the cursor line emits
  exactly one `No previous open task` notice and leaves the cursor unchanged;
- calling the method twice synchronously for a successful wrap does not jump twice;
- after the guard clears on the next macrotask, another call is accepted normally.

Keep existing behavior coverage:

- zero-task and one-task/current-line cases still produce the correct direction-specific notice once;
- one-task/not-current-line still jumps without a notice;
- multi-task wrap next/previous still lands on the expected circular target;
- parser coverage still confirms frontmatter/fence skipping and open `#task` status recognition.

### 4. Validation

Run static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Run the focused throwaway Node test and report the assertion count.

Review the task-related diff:

```bash
git -C /home/bryan/bob diff -- .obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob status --short
```

Manual Obsidian smoke test after reloading the plugin:

1. In a note with no open `#task` lines, press `<Ctrl+Shift+J>` once and confirm only one `No next open task` notice.
2. Press `<Ctrl+Shift+K>` once and confirm only one `No previous open task` notice.
3. In a note with exactly one open `#task`, put the cursor on that task and confirm each chord shows only one notice.
4. In a note with multiple open `#task` lines, confirm cycling still wraps correctly and does not skip a target.
5. Confirm insert-mode and non-Vim hotkey behavior still works.

### 5. Vault hygiene and commit

Before implementation edits, re-check:

```bash
git -C /home/bryan/bob status --short
```

Expected implementation file changed:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`

No expected changes:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/obsidian_vimrc.md`
- vault notes
- memory files
- Rust `bob-cli` files

After implementation, if approved, commit only the task-related plugin file with `/sase_git_commit`, leaving unrelated
dirty vault files untouched.

## Risks And Mitigations

- **Suppressing intentional repeat navigation:** Clear the guard on the next macrotask rather than using a visible
  debounce interval.
- **Still allowing double movement:** Place the guard before target calculation and cursor movement, not only around
  notice creation.
- **Breaking insert-mode hotkeys:** Keep the Obsidian hotkey binding and preserve the fallback's normal-mode check.
- **Leaking timers or stale editor keys:** Use a `WeakMap` keyed by editor object and clear direction marks promptly.
- **Over-scoping into task parsing:** Leave task recognition and circular target selection unchanged unless a test
  catches a regression.
