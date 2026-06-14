---
title: Obsidian Alt+O Child Bullet Open-Line Keymaps
create_time: 2026-06-14 18:21:18
status: proposed
prompt: sdd/prompts/202606/obsidian_alt_o_child_bullets_1.md
---

# Obsidian Alt+O Child Bullet Open-Line Keymaps

## Goal

Add two Vim normal-mode keymaps to Bryan's live Obsidian vault:

- `<Alt+o>` opens a new line below the current line.
- `<Alt+O>` / `<Alt+Shift+o>` opens a new line above the current line.

Both should behave like the existing Vim-mode `o` / `O` custom open-line mappings in mode and cursor behavior, but they
should always prefill the inserted line as a plain child bullet:

```text
  - |
```

The generated bullet must be one indentation level deeper than the current line, using two spaces per level, and the
cursor should land immediately after `- ` in Vim insert mode.

## Context Reviewed

- Required Obsidian memory was read with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and automation conventions before adding Obsidian keymaps"`.
- Workspace short memory `memory/short/sase.md` was reviewed.
- Live vault instructions `/home/bryan/bob/AGENTS.md` were reviewed. The vault is actively synced; implementation must
  inspect status before editing, avoid unrelated dirty files, and commit task-related vault edits with the SASE git
  workflow before terminating after edits.
- The live target is `/home/bryan/bob`, not the Rust `bob-cli` command surface.
- Current vault status has unrelated dirty note files only: `2026/20260614.md`, `bob.md`, `dev.md`, `sase.md`,
  `sase_blog.md`, and untracked `ref/chat/sase_audio_generation_consolidated.md`.
- Relevant existing implementation: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- `task-status-cycler` already owns raw CodeMirror Vim normal-mode mappings for custom `o` and `O`:
  - `o` -> `handleVimOpenLineBelow(cm)`
  - `O` -> `handleVimOpenLineAbove(cm)`
  - both insert with `replaceRange`, set the cursor, then enter insert mode via
    `window.CodeMirrorAdapter.Vim.handleKey(cm, "i", "mapping")`.
- The active vimrc file is `/home/bryan/bob/obsidian_vimrc.md`, but this task should not use vimrc dispatch because the
  desired behavior needs direct access to the CodeMirror Vim adapter object and an explicit insert-mode transition.
- Installed `obsidian-vimrc-support` uses CodeMirror Vim's `<A-...>` notation internally, including an existing `<A-y>s`
  mapping, so `<A-o>` is the expected notation for Alt+o.

## Behavior Specification

1. `<Alt+o>` inserts below.
   - Read the current line.
   - Compute child indentation from the current line's leading whitespace.
   - Insert `"\n" + childPrefix` at the end of the current line.
   - Move the cursor to the new line after the generated `- `.
   - Switch to Vim insert mode.

2. `<Alt+O>` inserts above.
   - Read the current line.
   - Compute the same child bullet prefix.
   - Insert `childPrefix + "\n"` at the start of the current line.
   - Move the cursor to the inserted line after the generated `- `.
   - Switch to Vim insert mode.

3. Child bullet prefix.
   - For normal two-space indentation, use: `currentLeadingSpaces + "  " + "- "`.
   - Examples:
     - `foo bar baz` -> inserted line `  - |`
     - `- foo bar baz` -> inserted line `  - |`
     - `  - foo bar baz` -> inserted line `    - |`
     - `    foo bar baz` -> inserted line `      - |`
   - The command always creates a plain `- ` bullet, never `- [ ] `, even when the current line is a task.
   - If the current line uses tabs or mixed leading whitespace, do not rewrite the existing line. For the generated
     prefix, keep behavior simple and deterministic: use the current leading whitespace plus two spaces, then `- `.

4. Preserve existing `o` / `O`.
   - Plain `o` and `O` should keep their current behavior, including task/list continuation.
   - The new Alt mappings are additive and separate.

5. Counts are out of scope for this change.
   - The existing custom `o` / `O` actions do not process Vim repeat counts; the new Alt actions should match that
     established behavior unless implementation discovers a no-risk way to share count handling.

## Implementation Scope

Expected vault file to edit:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

Expected files not to edit:

- `/home/bryan/bob/obsidian_vimrc.md`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- plugin manifests or community plugin configuration
- Markdown note content
- `bob-cli` Rust/source/docs/tests
- memory files

The plan artifact is `sase_plan_obsidian_alt_o_child_bullets.md` in the current `bob-cli` SASE workspace.

## Technical Design

1. Add a focused pure prefix helper near the existing open-line helpers:

```js
function getChildBulletOpenLinePrefix(lineText) {
  return `${getLineIndentation(lineText)}  - `;
}
```

This keeps the requested child-bullet behavior distinct from `getOpenLineBelowPrefix()`, which intentionally continues
tasks and bullets at the same level for plain `o` / `O`.

2. Export the new helper through `module.exports.helpers`.
   - This follows the plugin's existing testability pattern.
   - It allows a Node harness to assert prefix behavior without launching Obsidian.

3. Add two new Vim actions in `registerVimMappings()`:

```js
vim.defineAction("taskStatusCyclerOpenChildBulletLineBelow", (cm) => this.handleVimOpenChildBulletLineBelow(cm));
vim.defineAction("taskStatusCyclerOpenChildBulletLineAbove", (cm) => this.handleVimOpenChildBulletLineAbove(cm));
```

4. Register normal-mode Alt mappings:

```js
vim.mapCommand(
  "<A-o>",
  "action",
  "taskStatusCyclerOpenChildBulletLineBelow",
  {},
  {
    context: "normal",
  },
);
vim.mapCommand(
  "<A-O>",
  "action",
  "taskStatusCyclerOpenChildBulletLineAbove",
  {},
  {
    context: "normal",
  },
);
```

If live verification shows CodeMirror Vim reports Alt+Shift+o using a different token, add the narrow alias that matches
the live adapter rather than moving this to global Obsidian hotkeys.

5. Implement the two handlers by reusing the same editor guard, `replaceRange`, `setCursor`, and `enterVimInsertMode`
   shape as `handleVimOpenLineBelow()` and `handleVimOpenLineAbove()`, with only the prefix helper changed.

## Implementation Steps

1. Re-check state immediately before editing:
   - `git -C /home/bryan/bob status --short`
   - `git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js`
2. Add `getChildBulletOpenLinePrefix(lineText)`.
3. Add `handleVimOpenChildBulletLineBelow(cm)` and `handleVimOpenChildBulletLineAbove(cm)`.
4. Register the two Vim actions and map `<A-o>` / `<A-O>` with `{ context: "normal" }`.
5. Export the new helper in `module.exports.helpers`.
6. Validate statically and with a focused harness.
7. Reload Obsidian or the affected plugin and manually smoke-test the two keymaps.
8. Review the final vault diff and commit only the task file with the SASE git commit workflow, leaving unrelated dirty
   vault files untouched.

## Validation

Static checks:

```bash
node --check /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob status --short -- .obsidian/plugins/task-status-cycler/main.js
```

Focused Node harness checks with a mocked `obsidian` module and fake CodeMirror object:

- `getChildBulletOpenLinePrefix("foo bar baz")` returns `"  - "`.
- `getChildBulletOpenLinePrefix("- foo bar baz")` returns `"  - "`.
- `getChildBulletOpenLinePrefix("  - foo bar baz")` returns `"    - "`.
- `getChildBulletOpenLinePrefix("    foo bar baz")` returns `"      - "`.
- `getChildBulletOpenLinePrefix("- [ ] #task foo")` returns `"  - "`, not `"  - [ ] "`.
- Below handler inserts at end of current line, places cursor on the inserted line after the prefix, and calls insert
  mode.
- Above handler inserts before the current line, places cursor on the inserted line after the prefix, and calls insert
  mode.
- Vim registration defines both new actions and maps `<A-o>` / `<A-O>` only in normal context.
- Existing `o` / `O` mappings still point to the existing same-level continuation actions.

Manual Obsidian smoke test after reloading:

- In Vim normal mode on `foo bar baz`, `<Alt+o>` creates a line below with ` -` and enters insert mode after the prefix.
- In Vim normal mode on `- foo bar baz`, `<Alt+o>` creates a line below with ` -`.
- In Vim normal mode on `  - foo`, `<Alt+o>` creates a line below with `   -`.
- `<Alt+O>` performs the same insertion above the current line.
- On a task line, both mappings create a plain child bullet, not a child task.
- Plain `o` / `O`, `<CR>`, `<BS>`, `<C-d>`, `<C-u>`, and task toggle mappings still work.
- In Vim insert mode, Alt+o does not unexpectedly mutate the document through this normal-mode mapping.

## Risks And Mitigations

- Risk: CodeMirror Vim tokenizes `<Alt+Shift+o>` differently from `<A-O>` in this Obsidian runtime.
  - Mitigation: static registration uses the expected CodeMirror Vim notation; live smoke-test the chord. If needed, add
    only the confirmed alias.
- Risk: Alt+o is intercepted by the OS/window manager before CodeMirror receives it.
  - Mitigation: manual smoke test is required. If interception happens, report that the implementation is registered but
    the environment cannot deliver the chord, and choose a different chord only with user approval.
- Risk: changing `task-status-cycler` registration can break existing Vim mappings if a typo throws during setup.
  - Mitigation: keep the action additions adjacent to existing `o` / `O`, use known `mapCommand` patterns, and include a
    harness assertion for all relevant mappings.
- Risk: indentation edge cases with tabs or mixed whitespace.
  - Mitigation: keep the rule intentionally local: base the generated prefix on the current leading whitespace plus two
    literal spaces, without rewriting surrounding document indentation.

## Done Criteria

- `<Alt+o>` and `<Alt+O>` work in Vim normal mode in the live vault with the requested child bullet prefix and insert
  mode transition.
- Existing `o` / `O` continuation behavior is unchanged.
- Static checks and focused helper/handler mapping checks pass.
- The final vault diff is limited to `.obsidian/plugins/task-status-cycler/main.js`.
- Any vault edit is committed through the required SASE git commit workflow, with unrelated dirty vault files left
  untouched.
