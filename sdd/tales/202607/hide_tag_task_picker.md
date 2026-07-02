---
create_time: 2026-07-02 06:49:00
status: done
prompt: sdd/prompts/202607/hide_tag_task_picker.md
---
# Plan: Exclude `#hide` tasks from the `^^` task-link picker

## Product Context

The `^^` workflow is implemented by the **Block ID Prompt** Obsidian plugin in the linked `bob-plugins` repository:
`plugins/block-id-prompt/`. Typing `[[target^^]]` opens `TaskLinkPickerModal`, which currently receives tasks from
`collectTaskPickerItems(destination.content)`.

Today the picker lists every open `#task` in the target note, where "open" means status `[ ]`, `[/]`, or `[B]`. That is
too broad now that `#hide` is the vault's explicit "do not surface this task" marker. Hidden tasks should continue to
exist in the note, keep their block ids, and remain usable by other tooling, but they should not be offered as choices
in this menu.

This is a plugin behavior change only. It does not add or change any `bob-cli` subcommand or CLI option.

## Repository And Scope

Implementation belongs in the `bob-plugins` linked repository, not directly under `~/bob/`, because `bob-plugins` is the
source of truth for Bryan's custom Obsidian plugins. From a numbered `bob-cli` workspace, open the linked repo with:

```bash
sase workspace open -p bob-plugins -r "Need to edit block-id-prompt task picker filtering" <workspace_num>
```

Use the printed linked-repo path for all reads and writes. Paths below are relative to the `bob-plugins` repository
root.

In scope:

- `plugins/block-id-prompt/main.js`
- `plugins/block-id-prompt/manifest.json`
- `README.md`
- Deploying the changed plugin to the vault with `bob plugins sync`

Out of scope:

- Changing `bob-cli` Rust code.
- Changing the vault files directly.
- Changing the broader `bob-navigation-hotkeys` task navigation behavior unless a later request asks for that too.
- Hiding dependency-blocked tasks. Blocked marking is informational and unrelated to `#hide`.

## Current Code Shape

Relevant `block-id-prompt` helpers:

- `PROJECT_TASK_TAG_RE` matches standalone `#task`.
- `OBSIDIAN_TASK_LINE_RE` parses Markdown checkbox task lines.
- `taskItemFromLine(lineText, lineNumber)` currently accepts an item when:
  - the line is a Markdown checkbox task,
  - the status is in `OPEN_OBSIDIAN_TASK_STATUSES`, and
  - the body has standalone `#task`.
- `getOpenTasksInContent(content)` scans the file while skipping leading frontmatter and fenced code blocks, then calls
  `taskItemFromLine`.
- `collectTaskPickerItems(content)` builds a dependency index from all task lines, then enriches the open-task picker
  items.
- `openTaskLinkPicker(source)` calls `collectTaskPickerItems(destination.content)` and passes those items to
  `TaskLinkPickerModal`.

The cleanest boundary is therefore the picker item collector: hidden open tasks should be rejected before the modal sees
them. Dependency indexing should remain broader than visible picker rows so visible tasks can still be marked blocked by
hidden dependency tasks.

## Desired Behavior

A task line must be excluded from the `^^` picker when its parsed task body contains a standalone `#hide` tag,
regardless of where the tag appears in the body.

Examples excluded:

```md
- [ ] #task #prj Ship the project #hide ^prj
- [/] #task waiting on followup #hide [dependsOn:: blocker] ^next
- [B] #task #hide blocked helper
```

Examples still included:

```md
- [ ] #task Ship the project ^ship
- [/] #task waiting on followup [dependsOn:: blocker] ^next
```

Non-matches should remain non-matches:

- `#hidden` is not `#hide`.
- `#hideout` is not `#hide`.
- Done or cancelled tasks remain excluded for the existing open-status reason.
- Plain checkboxes without `#task` remain excluded.
- Task-shaped lines inside leading frontmatter or fenced code blocks remain excluded.

The picker subtitle should count only visible open tasks. If a note has open tasks but all of them are hidden, the
picker should display the existing zero-task empty state, for example `No open tasks in <basename>`. That copy is
acceptable because it means "no open tasks eligible for this picker."

## Implementation Design

1. Add a standalone hide-tag matcher near the existing task tag constants:

   ```js
   const HIDE_TASK_TAG_RE = /(^|[\s([{])#hide(?=$|[\s)\]},.;:!?])/;
   ```

   This mirrors the existing `#task` boundary behavior and avoids substring matches.

2. Add a small helper:

   ```js
   function hasHideTaskTag(text) {
     return HIDE_TASK_TAG_RE.test(String(text || ""));
   }
   ```

3. Update `taskItemFromLine(lineText, lineNumber)` so it returns `null` when the parsed task body has `#hide`.

   This keeps the filter exactly where picker rows are built, without changing the dependency index or low-level task
   parsing.

4. Consider updating `isOpenObsidianTaskLine(lineText)` only if it is used by picker-specific code after inspection. If
   it remains unused inside `block-id-prompt`, keep the behavior scoped to `taskItemFromLine` to avoid redefining "open
   Obsidian task" globally inside the plugin.

5. Update task display cleanup to strip `#hide` from any remaining display contexts where hidden tasks can still appear
   indirectly, especially dependency tooltips built from `buildTaskDependencyIndex(content)`.

   A conservative version is to generalize tag stripping from "strip `#task`" to "strip known internal task tags
   (`#task`, `#hide`)" while preserving spacing. This does not affect picker visibility, but keeps blocked-tooltips from
   showing internal marker noise when a visible task depends on a hidden task.

6. Leave `buildTaskDependencyIndex(content)` intentionally unfiltered by `#hide`.

   Reason: a hidden task can still be a real dependency target. If a visible task depends on a hidden open blocker, the
   visible task should still show the existing computed "Blocked" chip.

7. Bump `plugins/block-id-prompt/manifest.json` from `1.2.0` to `1.2.1` as a behavior-fix release.

8. Update the `README.md` plugin table row for Block ID Prompt:
   - version `1.2.1`
   - description mentions that the picker skips hidden tasks, without overloading the row with implementation detail.

## Validation

Run from the `bob-plugins` linked repo:

```bash
npm run validate
```

This verifies manifest shape and `node --check` syntax for every plugin. There is currently no dedicated unit-test
harness for these CommonJS Obsidian plugins because `main.js` imports Obsidian at module load.

Manual verification in the vault after deploy:

1. Create or use a scratch target note with mixed tasks:

   ````md
   - [ ] #task visible task ^visible
   - [ ] #task hidden task #hide ^hidden
   - [/] #task hidden in-progress #hide
   - [B] #task hidden blocked #hide
   - [x] #task done task ^done
   - [-] #task cancelled task ^cancelled
   - [ ] checklist without task tag

   ```tasks
   - [ ] #task fenced hidden #hide
   ```
   ````

   ```

   ```

2. From another note, type `[[scratch^^]]`.
3. Confirm the picker lists only `visible task`.
4. Confirm existing link completion still works for the visible task with an existing block id.
5. Add a visible task without a block id, pick it, and confirm the follow-up block-id prompt still appends the id and
   completes the source link.
6. Confirm a note with only hidden open tasks opens the picker with the empty state and does not show hidden rows.
7. Confirm a visible task that depends on a hidden open task still shows the existing computed blocked marker.

## Deployment

After validation, deploy the source-of-truth plugin to the vault from the `bob-plugins` linked repo:

```bash
bob plugins sync -p block-id-prompt -r "$PWD" --dry-run
bob plugins sync -p block-id-prompt -r "$PWD"
```

If sync reports that the managed vault plugin file is dirty, inspect the vault-side diff before using `--force`. Do not
edit the vault plugin copy directly.

## Risks And Mitigations

- **Boundary mistakes:** use the same standalone-token style as `#task` so `#hideout` and `#hidden` do not disappear.
- **Dependency regressions:** filter only picker rows, not the dependency index, so hidden blockers still participate in
  blocked-state calculation.
- **User confusion in empty state:** keep the existing empty state for now. If the distinction matters later, add a
  subtitle such as "0 visible open tasks" in a separate UX pass.
- **Version skew:** the vault will not see the change until `bob plugins sync` runs and the plugin is reloaded in
  Obsidian.
