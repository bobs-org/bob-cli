---
create_time: 2026-06-18 15:09:53
status: wip
prompt: sdd/prompts/202606/preserve_task_demote_properties.md
---
# Plan: Preserve All Inline Properties When Demoting Obsidian Tasks

## Context

The affected behavior is in Bryan's live Obsidian vault plugin:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- The hotkey is already registered in `/home/bryan/bob/.obsidian/hotkeys.json` as
  `task-status-cycler:toggle-obsidian-task` with `Ctrl+Shift+]`.

The current `Ctrl+Shift+]` command path is:

1. `handleToggleObsidianTaskCommand()` calls `getActiveObsidianTaskToggle()`.
2. `getActiveObsidianTaskToggle()` calls `getObsidianTaskToggleDocumentPlan()`.
3. The document planner calls `getObsidianTaskToggle()` for the active line.
4. If the line is a proper Obsidian task, `getDemoteObsidianTaskLineRewrite()` creates the normal-bullet rewrite.
5. That demotion helper removes the checkbox marker and then calls `collectObsidianTaskTokenRanges(body)`.

The unwanted behavior is in `collectObsidianTaskTokenRanges()` and its removal regex:

- It always removes standalone `#task`.
- It also removes inline Dataview properties whose keys are listed in `REMOVABLE_OBSIDIAN_TASK_PROPERTY_KEYS`, currently
  including `completion`, `due`, `scheduled`, `start`, `cancelled`, `priority`, `repeat`, `id`, `dependsOn`, and
  `onCompletion`.

That means this demotion:

```md
- [ ] #task Send followup [due::2026-06-20] [dependsOn:: abc123] [p::1] ^todo
```

currently becomes roughly:

```md
- Send followup [p::1] ^todo
```

The requested behavior is:

```md
- Send followup [due::2026-06-20] [dependsOn:: abc123] [p::1] ^todo
```

In other words, demotion should remove task syntax, not task metadata:

- Remove the checkbox marker.
- Remove standalone `#task`.
- Preserve every inline property, including properties that look specific to Obsidian Tasks.
- Preserve custom inline fields, links, normal text, spacing inside property brackets, child bullets, routing behavior,
  and trailing block IDs.

The vault currently has unrelated dirty note/settings files. The plugin target file is clean before this task. The
implementation should not touch unrelated vault files.

## Goal

Change the `Ctrl+Shift+]` Obsidian task demotion behavior so no Dataview inline properties are removed. This applies to
all demotion paths:

- In-place demotion when the active note is missing `Tasks` or `Future Work`.
- In-place demotion for non-routing-eligible list items, such as indented tasks.
- Move-plan demotion from `Tasks` to `Future Work` when both routing sections exist.

Promotion behavior should stay as it is today:

- Plain bullets still promote to `- [ ] #task ...`.
- Promotion still appends `[created::YYYY-MM-DD]` when no created field is present.
- Promotion still avoids adding a duplicate when a `[created::...]` field already exists.
- Block IDs remain the final token.

## Implementation

Edit only:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

1. Remove the demotion property-removal list and regex from the demotion token collector.
   - Delete `REMOVABLE_OBSIDIAN_TASK_PROPERTY_KEYS`.
   - Delete `REMOVABLE_OBSIDIAN_TASK_PROPERTY_RE`.
   - Remove the loop in `collectObsidianTaskTokenRanges()` that matches removable task properties.
   - Keep the standalone `#task` collection logic intact.

2. Keep `cleanObsidianTaskBody()` and `getDemoteObsidianTaskLineRewrite()` structurally unchanged.
   - They should still consume `collectObsidianTaskTokenRanges(body)`.
   - After the collector only returns standalone `#task` ranges, the existing whitespace compaction and metadata spacing
     normalization should preserve property fields without adding a new special case.

3. Keep promotion-specific created-field helpers unchanged.
   - `CREATED_FIELD_RE`, `lineHasCreatedField()`, `addCreatedFieldToObsidianTaskLine()`, and
     `getPromoteLineToObsidianTaskRewrite()` are still needed for the promotion side of the toggle.
   - This avoids regressing the previous "preserve created" fix and keeps the current task-creation contract.

4. Leave routing and editor-application code unchanged.
   - `getObsidianTaskToggleDocumentPlan()` already uses the demotion rewrite's `toggle.lineText` for both replace and
     move plans.
   - Once demotion preserves fields, both in-place and routed demotions inherit the behavior.

5. Do not change hotkeys, manifests, command IDs, or Vim mappings.
   - The request is about line rewrite behavior behind the existing `Ctrl+Shift+]` command, not key registration.

## Validation

1. Static checks:
   - `node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js`
   - `git -C /home/bryan/bob status --short -- .obsidian/plugins/task-status-cycler/main.js`

2. Focused Node helper assertions using the existing `module.exports.helpers` surface with a mocked `obsidian` module:
   - `demoteObsidianTaskLine("- [ ] #task Follow up [due::2026-06-20]")` returns `"- Follow up [due::2026-06-20]"`.
   - Demotion preserves all currently listed removable keys: `[completion::...]`, `[due::...]`, `[scheduled::...]`,
     `[start::...]`, `[cancelled::...]`, `[priority::...]`, `[repeat::...]`, `[id::...]`, `[dependsOn::...]`, and
     `[onCompletion::...]`.
   - Demotion also preserves arbitrary custom fields such as `[p::1]`, `[created::2026-06-18]`, and `[h:: abc]`.
   - Demotion still removes standalone `#task`.
   - Demotion does not remove non-standalone tags such as `#task/foo` or `prefix#task`.
   - Demotion keeps a trailing block ID after preserved fields.
   - `getObsidianTaskToggleDocumentPlan()` returns an in-place demotion that preserves properties for indented tasks.
   - `getObsidianTaskToggleDocumentPlan()` returns a move plan whose moved first line preserves properties when a
     top-level task is demoted from `Tasks` to `Future Work`.
   - Promotion of a bullet with an existing `[created::...]` field still avoids adding a duplicate.
   - Promotion of a bullet without a created field still appends the supplied test date.

3. Diff review:
   - Confirm the final vault diff is limited to `.obsidian/plugins/task-status-cycler/main.js`.
   - Confirm the diff removes only the property-removal code and related comments, with no unrelated rewrite of routing,
     cursor, keymap, or dependency-normalization logic.

4. Manual smoke test after reloading Obsidian or disabling/re-enabling `task-status-cycler`:
   - In a scratch note with `Tasks` and `Future Work`, demote a top-level task containing `#task`, `[due::...]`,
     `[dependsOn::...]`, `[priority::...]`, a custom field, and a block ID. It should move to `Future Work`, lose only
     the checkbox marker and standalone `#task`, and keep every property.
   - Demote an indented task with the same fields. It should convert in place and keep every property.
   - Promote the resulting bullets back to tasks and confirm `#task` and the checkbox return without duplicate created
     fields.

## Risks

- `collectObsidianTaskTokenRanges()` is exported through `module.exports.helpers`, so callers that directly assert its
  ranges will see fewer removal ranges. That is intentional because the helper's practical role is demotion cleanup.
- Keeping dependency fields like `[id::...]` and `[dependsOn::...]` on normal bullets may make some non-task bullets
  look task-derived, but the user explicitly wants no properties removed, including task-looking properties.
- Whitespace compaction still runs after `#task` removal. The plan keeps that behavior because it is about cleanup after
  removing the tag, not metadata deletion.
