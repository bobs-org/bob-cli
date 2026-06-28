---
create_time: 2026-06-28 12:24:21
status: wip
prompt: sdd/prompts/202606/dependency_block_id_batch_prompts.md
---
# Plan: Sequential block-ID prompts for batch dependency selection

## Repository

This feature belongs in the **`bob-plugins`** linked repo, under `plugins/bob-navigation-hotkeys/`. The implementation
should not touch the `bob-cli` Rust code or the vault plugin copy directly. After code changes are implemented and
validated, deploy the plugin through `bob plugins sync` as required by the linked repo instructions.

The current baseline is `bob-navigation-hotkeys` `1.6.0`, which already supports `<Tab>` batch add/remove for local task
dependencies and writes visible `**DEPENDS ON:** [[#^block-id]]` navigation child bullets.

## Goal

Allow the local-task dependency picker to include marked tasks that do not already have a trailing Obsidian block ID.
When a marked add needs a block ID, the picker should prompt the user for that block ID, one task at a time, then apply
the full batch once all required IDs have been provided.

This should preserve the existing strengths of the current flow:

- `<Tab>` remains a fast mark-and-advance gesture.
- `<Enter>` with no marks remains the single-task flow.
- Existing removals still work without asking for block IDs.
- `[dependsOn:: ...]` remains the canonical Tasks dependency field.
- Managed `**DEPENDS ON:** [[#^...]]` child bullets remain the human navigation layer.

## Terminology and identity rules

Use these definitions consistently throughout the feature:

- **Dependency value**: the value stored in `[dependsOn:: ...]`. Prefer an existing `[id:: value]`; otherwise use the
  task's trailing block ID; otherwise use the newly prompted block ID.
- **Navigation block ID**: the trailing `^block-id` that the visible `**DEPENDS ON:** [[#^block-id]]` bullet links to.
- **Needs block-ID prompt**: an unlinked task being added as a dependency whose current line has no trailing
  `^block-id`, even if it already has an `[id:: value]` field.

That last point is deliberate. If a task has `[id:: setup]` but no trailing `^setup`, it is already usable as a Tasks
dependency value, but it is still missing the block target needed by the navigation bullet. Prompt for the block ID,
prefill with `setup`, keep `[id:: setup]` unchanged, append the confirmed `^block-id`, and link the nav bullet to that
confirmed block ID.

For a task with neither `[id::]` nor `^block-id`, the confirmed ID should create both `[id:: confirmed]` and
`^confirmed`.

## Current behavior to extend

- `createBulletPropertyLocalTaskItems` builds rows from `getOpenLocalTasks`, including `existingIdField`,
  `existingBlockId`, `value`, `alreadyLinked`, and the current `needsBlockId` marker.
- `toggleHighlightedLocalTaskMark` currently refuses `needsBlockId` rows and shows a notice telling the user to press
  Enter for one-off block-ID creation.
- `commitMarkedDependencies` skips any marked item that needs a block ID.
- `chooseTaskDependency` handles the single-row path and can route through `showBlockIdStage` -> `confirmBlockId`.
- `confirmBlockId` immediately edits the target task, then calls `setLocalTaskDependency` for a one-task commit.

The new design should remove the batch limitation while reusing the existing block-ID stage rather than introducing a
second modal.

## Product behavior

### Marking

- `<Tab>` can mark unlinked rows even when they need a block ID.
- Marking an already-linked row still stages a removal and never prompts for a block ID.
- Marking an unlinked row with an existing trailing block ID stages a normal add.
- Marking an unlinked row without a trailing block ID stages an add that requires a later prompt.
- Toggling the same row again clears the mark.

The row badge should communicate the pending action:

- marked add with a block ID ready: `+ add`
- marked add needing a block ID: `+ id`
- marked remove: `- remove`
- unmarked linked/existing/create rows keep the existing meanings

### Applying

When the user presses `<Enter>` with marks staged:

1. If no marked additions need a block ID, apply the batch immediately using the current add/remove reconciliation flow.
2. If one or more marked additions need a block ID, enter the block-ID prompt stage for the first such task.
3. After each valid block ID is confirmed, prompt for the next task that needs one.
4. After the final prompt is confirmed, apply the entire batch once.

No target task lines, cursor bullet, `dependsOn` field, or navigation child bullets should be modified until all
required block IDs have been collected. If the user closes the modal during the prompt sequence, cancel without writing
anything.

### Single-task flow

With no marks staged, `<Enter>` should keep the familiar single-selection behavior, but it should use the same stricter
"missing trailing block ID requires a prompt" rule:

- task has `[id:: setup]` but no `^block`: prompt, prefilled with `setup`; after confirm, keep `[id:: setup]`, append
  the chosen `^block`, store `setup` in `dependsOn`, and link navigation to the chosen block.
- task has neither `[id::]` nor `^block`: prompt, then create `[id:: chosen] ^chosen`, store `chosen`, and link
  navigation to `chosen`.
- task has a trailing block ID: no prompt; continue through the existing identity resolution.

## Technical design

### 1. Clarify row metadata

Keep `getLocalTaskDependencyIdentifier(task)` for the dependency value, but add explicit metadata so the UI and commit
path do not overload "has any identifier" with "has a block link":

- `dependencyValue`: `existingIdField || existingBlockId || ""`
- `linkBlockId`: `existingBlockId || ""`
- `needsBlockIdPrompt`: `!existingBlockId`
- `needsDependencyValue`: `!dependencyValue`
- `needsPromptForAdd`: `!alreadyLinked && needsBlockIdPrompt`

The old `needsBlockId` name can either be replaced or kept as an alias during the refactor, but the final code should
make it clear that the prompt is about the trailing block link needed for navigation.

### 2. Add batch prompt state

Extend `BulletPropertyPickerModal` with a small pending-batch object used only while the modal is collecting block IDs:

- `propertyName`
- original cursor line and cursor position
- marked task snapshots, keyed by task line
- removal task snapshots
- add task snapshots that already have a block ID
- prompt queue for add task snapshots missing a block ID
- confirmed block IDs by task line
- reserved block IDs chosen earlier in the same prompt sequence

Clear this state when returning to the property stage, returning to the local-task value stage, closing the modal, or
finishing/cancelling the prompt sequence.

### 3. Refactor block-ID prompting

Refactor `showBlockIdStage` and `confirmBlockId` so they can serve both single and batch modes.

For batch mode:

- subtitle should show progress, such as `Block ID 2 of 4 · line 37`
- prefill should be the existing `[id::]` value when present; otherwise use `suggestBlockIdFromTask`
- suggestions and validation must account for IDs already confirmed earlier in the same batch
- confirming an intermediate prompt records the ID and opens the next prompt without closing the modal
- confirming the final prompt calls the batch executor and closes only if the executor succeeds

For single mode:

- preserve the one-task flow, but route through the same "apply prompted block ID to target line" helper

### 4. Add or adjust pure helpers

Add focused helpers and export them through `module.exports.helpers` for Node checks:

- `taskNeedsPromptedBlockId(task)` or equivalent metadata helper for row creation and commit planning.
- `suggestBlockIdFromTask(..., { reservedIds })` or a companion helper so suggestions avoid IDs chosen earlier in a
  pending batch.
- `validateBlockIdCandidate(id, content, { reservedIds })` so duplicates in the current note and duplicates within the
  same prompt sequence are both rejected.
- `applyPromptedBlockIdToTaskLine(line, id)` that appends `^id` and inserts `[id:: id]` only when the task lacks an
  existing `[id::]` value.
- `resolveTargetTaskIdentity` can either gain an option for "prompt when trailing block ID is missing" or be split so
  the prompted and non-prompted cases are explicit.

Keep the existing dependency-list and navigation-bullet helpers intact unless the new execution flow needs small
metadata additions.

### 5. Prepare before writing, execute once

Reshape `commitMarkedDependencies` into two phases.

Preparation phase:

- re-read and guard the cursor bullet before entering prompt mode
- partition marked rows into removals, ready additions, and additions requiring prompts
- if prompts are required, store the pending batch and show the first prompt; return `false` so the modal stays open
- if no prompts are required, call the executor immediately

Execution phase:

- re-read and guard the cursor bullet again before any write
- for removals, remove the dependency value and delete the matching managed navigation bullet
- for ready additions, re-read target lines, stale-check them, resolve identity, and apply any target-line edits
- for prompted additions, re-read target lines, stale-check them, apply the confirmed block ID, and derive:
  - `depValue = existing [id::] || confirmed block ID`
  - `linkBlockId = confirmed block ID`
- rewrite the cursor bullet's `[dependsOn:: ...]` list once via `applyLocalTaskDependencyListEdits`
- reconcile navigation bullets last, re-planning against fresh editor content after each line-changing insert/delete
- restore the cursor and show one consolidated notice

Partial success should match the current batch philosophy: stale target additions are skipped and reported, while other
valid additions/removals still apply. A stale or changed cursor bullet aborts the whole batch before any write.

### 6. UI updates

Update `renderTaskValueItem`, subtitle counts, and CSS:

- allow marked block-ID-needed rows to render as marked adds with a distinct `is-marked-id-needed` style
- include "needs ID" or `+ id` in the badge so the prompt requirement is visible before applying
- update the local-task subtitle to distinguish `N to add`, `M need IDs`, and `R to remove`
- update footer hint text only if needed; the current dynamic `Enter: Apply` behavior can remain
- update block-ID preview text to show whether confirmation will:
  - add `[id:: chosen] ^chosen`, or
  - append `^chosen` while preserving existing `[id:: value]`

### 7. Metadata and docs

- Bump `plugins/bob-navigation-hotkeys/manifest.json` from `1.6.0` to `1.7.0`.
- Update the `bob-navigation-hotkeys` row in `README.md` to mention batch dependency selection with sequential block-ID
  prompts.

## Edge cases

- **Multiple ID-less marked tasks**: prompt in mark order, validate each against note content and earlier prompted IDs.
- **User closes during prompts**: no writes have occurred, so cancellation is clean.
- **Existing `[id::]` but no trailing block ID**: prompt for the block ID, keep `[id::]` as the dependency value.
- **No `[id::]` and no trailing block ID**: prompt once and create both representations from the confirmed ID.
- **Already-linked rows without a trailing block ID**: if marked for removal, remove only the dependency value and any
  matching managed navigation bullet that can be identified; do not prompt.
- **Filtered marks**: keep existing behavior where marks persist across filtering and are applied even if hidden.
- **Duplicate prompted IDs**: reject duplicates before execution; also guard again at execution in case the note changed
  while the prompts were open.
- **Target line changed while prompting**: skip that target and report it in the consolidated notice.
- **Cursor bullet changed while prompting**: abort the whole batch before making any writes.
- **Mixed add/remove with prompts**: collect all prompts first, then perform one list reconciliation and nav-bullet
  pass.
- **Existing dependency IDs not visible in the picker**: preserve them unless the user explicitly marked a visible row
  for removal.

## Validation

Run from the `bob-plugins` checkout:

- `node --check plugins/bob-navigation-hotkeys/main.js`
- focused Node checks for the new pure helpers:
  - validation rejects duplicates in note content
  - validation rejects duplicates reserved earlier in the same batch
  - suggestions avoid both existing and reserved IDs
  - prompted line update creates `[id:: chosen] ^chosen` for a task with neither value
  - prompted line update appends `^chosen` but preserves `[id:: existing]`
  - dependency value and navigation block ID split correctly for `[id:: existing] ^chosen`
- in-memory modal/editor checks for:
  - marking a blockless task, confirming one prompt, and applying the dependency/nav link
  - marking two blockless tasks and receiving prompts one at a time
  - mixing ready additions, prompted additions, and removals in one batch
  - cancelling before final prompt causes no editor writes
  - stale target during prompt sequence is skipped and reported
- `npm run validate`
- `git diff --check`
- `bob plugins sync --dry-run --repo <linked-bob-plugins-checkout> --plugin bob-navigation-hotkeys`
- after the dry run shows only the expected plugin files, run the real scoped
  `bob plugins sync --repo ... --plugin bob-navigation-hotkeys`
