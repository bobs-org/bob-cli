---
create_time: 2026-06-15 08:00:33
status: done
prompt: sdd/prompts/202606/obsidian_child_bullet_tab_indent.md
---
# Fix Obsidian Child-Bullet Indentation

## Goal

Make the `Option+o` / `Option+Shift+o` child-bullet commands generate source text that Obsidian treats like a normal
nested bullet created manually with Tab indentation.

The generated child bullet should match the user's expected workflow:

- create a blank line;
- type `-`;
- press Tab until the list item is at the desired child level;
- continue typing after the generated bullet marker.

For the common top-level case, the generated source should be `\t- `, not ` -`.

## Context Reviewed

- Workspace short-term SASE memory was reviewed from `memory/short/sase.md`.
- Required Obsidian memory was read with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and plugin context before planning a fix for generated bullet indentation"`.
- Live vault instructions were reviewed from `/home/bryan/bob/AGENTS.md`.
- Prior SDD `/sdd/tales/202606/obsidian_alt_o_below_keymap_fix_1.md` was reviewed. It intentionally specified "current
  leading whitespace plus two spaces plus `- `", which now appears to be the source of the rendering issue.
- The live target plugin is `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- The live plugin file is currently clean relative to git. The vault has unrelated dirty note files that must not be
  reverted, staged, or committed.
- Existing vault notes commonly use tab-indented child bullets and task children, for example lines matching `^\t+- `
  and `^\t+- [ ] `. The plugin also already uses `EMPTY_POMODORO_SUB_BULLET_LINE = "\t- "`.
- The current helper is: `getChildBulletOpenLinePrefix(lineText) { return `${getLineIndentation(lineText)} - `; }`

## Diagnosis

The previous fix solved event delivery for plain `Option+o`, but the insertion helper still emits a two-space child
indent. That matched the previous SDD, but it does not match the user's observed Obsidian behavior or the vault's
dominant nested-list source style.

The smallest likely correct fix is to change only the child-bullet indentation unit from two spaces to a literal tab.
This preserves the rest of the hotkey path:

- Vim normal-mode gating;
- focused-editor checks;
- `beforeinput` fallback for macOS `Option+o`;
- cursor placement after `- `;
- transition into Vim insert mode;
- existing `Option+Shift+o` above behavior.

## Files Expected To Change

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

## Files Expected Not To Change

- `/home/bryan/bob/.obsidian/hotkeys.json`
- Markdown notes under `/home/bryan/bob`
- Memory files
- `bob-cli` source files, except this proposed SASE plan file in the current workspace

## Implementation Plan

1. Re-check live vault state before editing.
   - Run `git -C /home/bryan/bob status --short --branch`.
   - Confirm `.obsidian/plugins/task-status-cycler/main.js` has no unrelated user edits.
   - Confirm `hotkeys.json` still contains the two child-bullet bindings, without modifying it.

2. Update the child-bullet prefix helper.
   - Add a small named constant near the existing list constants, such as `const CHILD_BULLET_INDENT_UNIT = "\t";`.
   - Change `getChildBulletOpenLinePrefix(lineText)` to emit:
     `getLineIndentation(lineText) + CHILD_BULLET_INDENT_UNIT + "- "`.
   - Update the nearby comment so it says the command emits one Obsidian Tab-indent level deeper than the current line,
     not two spaces.
   - Do not change `getOpenLineBelowPrefix()`, task continuation behavior, or any keydown/beforeinput routing logic.

3. Keep normalization deliberately narrow.
   - Preserve the current line's existing leading whitespace and append one tab for the child level.
   - Do not rewrite existing note content or try to clean up previously generated ` -` lines in this task.
   - If mixed-indentation parent lines exist, the new child line may inherit that parent prefix plus one tab. That is a
     safer first fix than silently normalizing existing indentation in live notes.

4. Add focused validation around the helper.
   - Use a temporary Node harness or existing helper export to assert:
     - `getChildBulletOpenLinePrefix("- [ ] task") === "\t- "`;
     - `getChildBulletOpenLinePrefix("- bullet") === "\t- "`;
     - `getChildBulletOpenLinePrefix("\t- child") === "\t\t- "`;
     - `getChildBulletOpenLinePrefix("\t\t- grandchild") === "\t\t\t- "`;
     - non-list indented lines still preserve their current leading whitespace and add exactly one tab.
   - Include below/above handler assertions that cursor placement remains `childPrefix.length` after insertion.

5. Re-run the previous event-path harness if practical.
   - Verify that `Option+o` below and `Option+Shift+o` above still dispatch through the same normal-mode-only path.
   - The expected inserted prefixes should now contain tabs instead of two spaces.
   - Keep insert/visual/replace mode fallthrough assertions.

6. Run static checks.
   - `node --check /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`

7. Manual smoke test in Obsidian after reloading the plugin.
   - In Vim normal mode on a top-level task, `Option+o` creates a child line below whose source begins with `\t- ` and
     renders as a proper Obsidian child bullet.
   - `Option+Shift+o` creates the same tab-indented child bullet above.
   - Running the command from an already tab-indented bullet adds one more tab.
   - Insert/visual/replace modes still fall through.
   - Plain `o` / `O` and other task-status-cycler keymaps are unaffected.

8. Review and commit after implementation.
   - Confirm the final vault diff is limited to `.obsidian/plugins/task-status-cycler/main.js`.
   - Stage only that file.
   - Commit through the required SASE git commit workflow.
   - Leave unrelated dirty note files untouched.

## Risks And Mitigations

- Risk: Some existing imported notes use two-space child indentation.
  - Mitigation: preserve existing parent indentation and only change the extra child-indent unit generated by this
    command.
- Risk: A broader normalization pass could corrupt intentional spacing in notes.
  - Mitigation: do not modify note content or normalize existing whitespace in this task.
- Risk: The fix changes helper behavior used by both above and below commands.
  - Mitigation: both commands are supposed to create the same child bullet shape; test both insertion paths.
- Risk: The source renders correctly only after a plugin reload.
  - Mitigation: include an explicit manual Obsidian reload/smoke-test step.

## Done Criteria

- Generated child bullets use tab indentation and render as proper Obsidian nested bullets.
- `Option+o` creates the child bullet below and enters insert mode.
- `Option+Shift+o` creates the child bullet above and enters insert mode.
- Existing normal-mode-only gating and fallthrough behavior is preserved.
- Static checks and focused harness validation pass.
- The vault plugin change is committed with unrelated dirty notes untouched.
