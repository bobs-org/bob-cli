---
create_time: 2026-06-14 07:14:07
status: done
prompt: sdd/prompts/202606/project_note_child_bullets_to_tasks.md
---
# Plan: Convert Promoted Task Child Bullets Into Project Tasks

## Goal

Change the Obsidian `<Ctrl+Alt+Shift+N>` project-promotion flow so a source task with child bullets no longer aborts.
When the selected open `#task` is promoted to a new project note:

1. The selected task still seeds the project `^prj` completion-criteria task.
2. Its child bullets are moved into the new project note's `## Tasks` section as real Obsidian Tasks.
3. The template's default placeholder task under `## Tasks` is removed from the created note whenever child tasks are
   inserted.
4. The source task block is removed from the source note only after the new project note has been seeded and any block
   link rewrites have succeeded.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian task and project-note workflow context before planning task promotion changes"`.
- Plan workflow from `/home/bryan/.codex/skills/sase_plan/SKILL.md`.
- Live vault instructions: `/home/bryan/bob/AGENTS.md`.
- Current live implementation: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- Current live project template: `/home/bryan/bob/_templates/new_project.md`.
- Prior plans/tales:
  - `sdd/tales/202606/obsidian_project_from_task_keymap.md`
  - `sdd/tales/202606/obsidian_project_from_task_block_id.md`
  - `sdd/tales/202606/project_note_default_filenames.md`
  - `sdd/tales/202606/project_task_links_to_prj.md`
  - `sdd/tales/202606/obsidian_ctrl_shift_bracket_task_toggle.md`

No `bob-cli` subcommands or options are being added, so `memory/long/cli_rules.md` is not required.

## Current Findings

- The command is already registered as `bob-navigation-hotkeys:create-project-note-from-task` and bound to
  `<Ctrl+Alt+Shift+N>`.
- `createProjectNoteFromTask()` currently aborts before creating anything when
  `hasMoreIndentedChildListItem(editor, cursor.line, lineText)` returns true.
- `buildProjectContentFromTask(content, parsedTask)` only seeds the `^prj` placeholder and priority.
- `removeTaskLineFromContent(content, lineNumber, lineText)` removes only the selected source task line, which was safe
  while child bullets were disallowed.
- The template's Tasks section currently renders:

  ```md
  ## Tasks

  - [ ] #task (REPLACE WITH TASK DESCRIPTION) [created::<templater date>]
  ```

- The live vault is dirty only in normal note files right now; the plugin, template, and hotkey files are clean. The
  implementation must still re-check status immediately before editing because the vault is actively synced.

## Behavior

### Source Capture

Replace the boolean child-bullet abort with a source task block capture:

- Parse the selected task line exactly as today: it must be an open checkbox list item carrying standalone `#task`.
- Determine the selected task's list indentation.
- Capture the contiguous child block below it: all following lines that are blank or have indentation deeper than the
  selected task, stopping at the first nonblank line whose indentation is less than or equal to the selected task's
  indentation, or EOF.
- Keep the selected parent line separate from the captured child block. The parent task continues to seed the project
  `^prj` task; it is not duplicated into the new note's `## Tasks` section.

### Child Bullet Conversion

Convert direct child list items into top-level project tasks under `## Tasks`:

- Plain child bullets become open tasks: `\t- Call Alice` -> `- [ ] #task Call Alice [created::YYYY-MM-DD]`.
- Child checkbox bullets preserve their existing checkbox status when present: `\t- [/] Draft outline` ->
  `- [/] #task Draft outline [created::YYYY-MM-DD]`.
- Add a standalone `#task` token unless the child line already has one.
- Add `[created::YYYY-MM-DD]` unless the child line already has a `created` inline field.
- Preserve useful content such as links, inline fields, formatting, and trailing block IDs.
- Convert only direct child list items into sibling top-level tasks in the new project note. If a direct child has its
  own nested lines, keep those nested lines under the converted task as supporting sub-bullets/continuations, normalized
  one level deeper than the new top-level task.

This avoids data loss while keeping the project Tasks section queryable by the Tasks plugin.

### Template Placeholder Removal

When converted child tasks are present:

- Remove the template's default placeholder task line from the created project's `## Tasks` section.
- Do this during the same `vault.process(createdFile, ...)` rewrite that seeds the `^prj` task and inserts converted
  child tasks.
- Do not edit `_templates/new_project.md`; ordinary `<Ctrl+Shift+N>` project creation should continue to render the
  placeholder task.

The removal should target only a task line inside the `## Tasks` section whose body contains the known placeholder
`(REPLACE WITH TASK DESCRIPTION)`, not arbitrary user text elsewhere in the file.

### Source Deletion

When child bullets were captured:

- Remove the whole captured source task block from the source note after project creation succeeds.
- The block removal must use the originally captured parent line, start line number, and child block extent.
- If the exact block no longer matches at the original location, fall back only to a unique exact block match.
- If no safe match exists, keep the source block and show a warning toast, matching today's conservative source-line
  deletion behavior.

Tasks without child bullets should keep today's effective behavior, though they can flow through the same block-removal
helper with a one-line block.

## Design

Keep the change localized to `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

Add constants:

- `PROJECT_TASKS_PLACEHOLDER = "(REPLACE WITH TASK DESCRIPTION)"`.
- `PROJECT_TASKS_HEADER = "## Tasks"` or reuse the existing `DASH_TASKS_HEADER` only if the name still reads clearly.

Add pure helpers near the existing project-task helpers and export them through `module.exports.helpers`:

- `getProjectSourceTaskBlock(editor, lineNumber, parentLineText)`: returns
  `{ startLine, endLineExclusive, lines, childLines }` or `null`.
- `parseProjectChildListItem(lineText, parentIndentLength)`: identifies direct child list items, extracts
  marker/checkbox/body/status, and returns normalized metadata.
- `buildProjectTasksFromChildBullets(childLines, createdDateString)`: returns an array of rendered Markdown lines for
  the new project's `## Tasks` section.
- `buildProjectTaskLineFromChildBullet(parsedChild, createdDateString)`: adds checkbox marker, `#task`, and
  `[created::YYYY-MM-DD]` while preserving existing task metadata.
- `replaceProjectTasksPlaceholder(content, renderedTaskLines)`: finds the `## Tasks` section outside frontmatter/fenced
  code, removes the default placeholder task when needed, and inserts the rendered child tasks in that section.
- `buildProjectContentFromTask(content, parsedTask, options)`: extends the existing helper to seed `^prj`, apply
  priority, and optionally insert converted child tasks.
- `removeTaskBlockFromContent(content, block)`: replaces `removeTaskLineFromContent` for this command while preserving
  exact/unique fallback semantics.
- `formatProjectTaskCreatedDate(date)`: small local `YYYY-MM-DD` helper, matching the compact `[created::YYYY-MM-DD]`
  convention used elsewhere.

Command flow changes:

1. Parse the selected parent task as today.
2. Capture the source task block and render child tasks if present.
3. Remove the `hasMoreIndentedChildListItem` abort.
4. Keep the existing block-ID derived filename, collision check, backlink rewrite snapshot, `view.save()`, note
   creation, placeholder seeding, block-link rewrite, and success toast flow.
5. During the created-note rewrite, pass the rendered child tasks into `buildProjectContentFromTask`.
6. During the source-note rewrite, remove the captured block rather than only the parent line.

## Edge Cases

- Source task has no child bullets: no placeholder removal in `## Tasks`; existing default project-task behavior
  remains.
- Source task has child bullets but no valid direct child list items after parsing: keep the placeholder task and remove
  the whole source block only if no content would be lost. If parsing finds unsupported child content, prefer warning
  and keeping the source block over silent loss.
- Source task has blank lines within its child block: preserve meaningful grouping under converted tasks where possible,
  but do not create empty tasks.
- Child bullet already has `#task`: do not duplicate the tag.
- Child bullet already has `[created::...]`: do not append another created field.
- Child bullet has a trailing block ID: preserve it on the converted task.
- Child bullet is ordered: the converted task should use normal unordered task syntax because the destination is a task
  list, not an ordered outline.
- Created project note lacks `## Tasks`: seed `^prj`, keep the source block, and show a warning because there is no safe
  insertion point for child tasks.
- Created project note lacks the `^prj` placeholder: keep today's behavior, warn, and keep the source block.
- Block-ID backlink rewrite failure: keep today's behavior, meaning the source block is kept so old block links still
  resolve.

## Validation

Static checks:

```bash
node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Focused Node helper checks with a mocked `obsidian` module:

- Existing no-child task still seeds `^prj`, leaves the template placeholder task, and removes only the parent source
  task line.
- Plain child bullets convert to top-level `- [ ] #task ... [created::YYYY-MM-DD]` lines under `## Tasks`.
- Existing child checkbox status is preserved.
- Existing child `#task`, `[created::...]`, custom inline fields, links, and trailing block IDs are preserved without
  duplication.
- The default `(REPLACE WITH TASK DESCRIPTION)` task is removed when converted child tasks are inserted.
- Nested content under a direct child remains nested below that converted task.
- Source deletion removes the full parent-plus-child block, preserves surrounding lines, and handles CRLF input.
- Exact-block mismatch keeps the source block and returns `removed: false`.
- Missing `## Tasks` or missing placeholder keeps the source block.
- Block-ID task with child bullets still creates the block-ID-derived project note, rewrites backlinks to `^prj`, and
  only then removes the source block.

Manual acceptance after reloading Obsidian:

- From an area/project note, promote an open task with two plain sub-bullets. The new project note should have the
  parent text on `^prj`, two real tasks under `## Tasks`, no placeholder task under `## Tasks`, and the original source
  block should be gone.
- Promote an open task with no sub-bullets. Behavior should match the current flow, including the default placeholder
  task in `## Tasks`.
- Promote a task with a trailing block ID and at least one backlink. Backlinks should point to the new project's `^prj`
  task and the source block should be removed only after the rewrite succeeds.

## Scope

Expected file to edit:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`

Expected files not to edit:

- `/home/bryan/bob/_templates/new_project.md`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
- Any `bob-cli` Rust source
- Any memory files

## Commit Workflow

Before implementation, re-check:

```bash
git -C /home/bryan/bob status --short
```

After implementation, inspect the vault diff and commit only `.obsidian/plugins/bob-navigation-hotkeys/main.js` via the
`/sase_git_commit` skill, as required by `/home/bryan/bob/AGENTS.md`.
