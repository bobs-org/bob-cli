---
create_time: 2026-06-28 10:18:59
status: done
prompt: sdd/prompts/202606/obsidian_dependency_navigation_subbullet.md
---
# Plan: Add visible dependency block-link child bullets to the local task dependency picker

## Repository

This feature lives in the **`bob-plugins`** linked repo, in `plugins/bob-navigation-hotkeys/`. The current `bob-cli`
workspace only holds this plan artifact.

No `bob-cli` Rust changes and no `chezmoi` config changes are needed: the already-implemented `dependsOn: local_task_id`
config remains the trigger for this behavior.

## Goal

Extend the `Ctrl+Shift+P` bullet-property picker's `dependsOn` / `local_task_id` flow so that, when the user links the
current bullet to a local task dependency, the plugin also adds a readable child bullet:

```markdown
- **DEPENDENCIES:** [[#^<block_id>]]
```

The existing `[dependsOn:: <id>]` inline property remains the canonical Tasks-compatible dependency. The new child
bullet is a human navigation affordance: it gives the user a clickable Obsidian block link to the target dependency
task.

I am interpreting "currently selected note" as the bullet line under the cursor in the active note, because that is the
current behavior of the `set-bullet-property` command. The dependency picker still only targets open `#task`s in the
same file, so `[[#^<block_id>]]` is the correct local block-link form.

## Product behavior

When a dependency is newly added:

```markdown
- [ ] #task Current task [dependsOn:: target-id]
  - **DEPENDENCIES:** [[#^target-id]]
- [ ] #task Target task [id:: target-id] ^target-id
```

If the target task already has both `[id:: task-id]` and `^block-id`, and they differ, preserve the prior dependency
semantics:

- `[dependsOn:: task-id]` continues to use the `[id::]` value so Obsidian Tasks queries resolve correctly.
- the visible child bullet uses the actual block ID: `[[#^block-id]]`, because the link must navigate to the block.

If the target has `[id:: id]` but no block ID, the existing resolver appends `^id`; the visible link uses `id`. If the
target has `^id` but no `[id::]`, the existing resolver adds `[id:: id]`; the visible link uses `id`. If the target has
neither, the block-ID creation stage creates both and the child link uses the newly created block ID.

## Key decisions

- Add the visible link only as a child bullet under the current bullet, not as another inline property. This keeps Tasks
  compatibility separate from navigation UX.
- Make insertion idempotent. Re-running the picker for the same dependency must not create duplicate `**DEPENDENCIES:**`
  child bullets.
- Keep one child bullet per dependency link, matching the requested shape, instead of merging all links onto one line.
- Insert dependency child bullets as a grouped block: if dependency bullets already exist, append after the last one;
  otherwise insert immediately after the parent bullet, before other child notes.
- Use one logical child indent. For a top-level bullet this renders exactly as `  - ...`; for an already-indented
  parent, append the same child indent to the parent indentation.
- Preserve the existing `[dependsOn:: a, b]` merge behavior. The new child bullet is a companion write, not a
  replacement for the existing machine-readable property.
- Treat already-linked picker selections as an idempotent repair path: if `[dependsOn:: id]` is already present but the
  visible child link is missing, selecting the task should add the child link. If both already exist, show a no-op
  notice.

## Technical design

### 1. Extend target identity resolution with a link block ID

Update the pure target-resolution path in `plugins/bob-navigation-hotkeys/main.js` so callers can distinguish:

- `dependencyValue`: the value written to `[dependsOn:: ...]` (usually the block ID, but existing `[id::]` wins for
  Tasks compatibility), and
- `linkBlockId`: the actual block ID to use in `[[#^...]]`.

The existing `resolveTargetTaskIdentity(line)` can either grow a `linkBlockId` return field or be wrapped by a new
helper with clearer naming. The important cases:

- existing `[id:: id]`, no block ID -> target edit appends `^id`; `dependencyValue = id`; `linkBlockId = id`.
- existing `^block`, no `[id::]` -> target edit adds `[id:: block]`; `dependencyValue = block`; `linkBlockId = block`.
- existing `[id:: id]` and `^block` -> no rewrite; `dependencyValue = id`; `linkBlockId = block`.
- neither -> block-ID stage supplies both values from the newly accepted block ID.

### 2. Add pure helpers for dependency child bullets

Add helpers near the existing bullet-property/task helpers and export them through `module.exports.helpers` for ad-hoc
Node checks:

- `formatDependencyNavigationBullet(blockId, indent)` -> `"<indent>- **DEPENDENCIES:** [[#^<blockId>]]"`.
- `parseDependencyNavigationBullet(line)` -> block ID when a line exactly matches the managed shape, otherwise `null`.
- `getBulletIndent(line)` -> leading whitespace for a list item.
- `getDependencyChildIndent(lines, parentLine)` -> existing direct-child indent if there is one, otherwise parent indent
  plus two spaces.
- `findCurrentBulletChildBlock(lines, parentLine)` -> `{ startLine, endLineExclusive }`, scanning until the first
  nonblank line at the parent indentation or shallower. This can mirror the existing `getProjectSourceTaskBlock`
  approach.
- `planDependencyNavigationBulletInsertion(content, parentLine, blockId)` -> an immutable result:
  - `alreadyPresent: true` when a managed child bullet already links to that block ID.
  - `insertLine` and `lineText` when insertion is needed.
  - `reason` for guard failures such as "parent is not a bullet" or "empty block ID".

The planner should look only inside the current bullet's child block, not the whole file. That prevents a link under a
different task from suppressing the link under the selected task.

### 3. Add an editor insertion helper

Add a small editor helper beside `replaceEditorLine`:

- `insertEditorLine(cm, line, lineText)` or a similarly named function that inserts a full line at a line boundary.

It should handle both middle-of-file insertion and appending after the final line. This avoids overloading
`replaceEditorLine` with multi-line replacement for a different operation.

### 4. Update the dependency write flow

Change `setLocalTaskDependency(cm, cursor, name, id, options = {})` to accept a `linkBlockId` option.

The method should:

1. Re-read the current bullet line.
2. Merge `[dependsOn:: id]` with `upsertLocalTaskIdValue`, preserving existing behavior.
3. If the merge changed the line, replace the current line.
4. If `linkBlockId` is present, plan and apply the child-bullet insertion against the updated editor content.
5. Restore the cursor to the current bullet line.
6. Show a notice that distinguishes:
   - dependency and navigation link added,
   - dependency already existed but navigation link added,
   - both already existed,
   - guard failure while adding the navigation link.

The target task line is already updated before `setLocalTaskDependency` runs. Keeping the child-bullet insertion last is
important because it may shift later line numbers; by then the target line edit is complete.

### 5. Update picker selection paths

In `chooseTaskDependency(item)`:

- Do not immediately return on `item.alreadyLinked`.
- Re-read and verify the target task line just as the current code does.
- Resolve the target identity, apply any target edit, then call `setLocalTaskDependency(..., { linkBlockId })`.
- If the dependency field was already present, let `setLocalTaskDependency` decide whether a visible link still needed
  to be inserted.

In `confirmBlockId(item)`:

- After creating `[id:: id] ^id` on the target task, call `setLocalTaskDependency(..., { linkBlockId: item.id })`.
- Keep the existing success notice concise, but include that the link was added when applicable.

### 6. Metadata and docs

Because this is a visible behavior change in an Obsidian plugin:

- bump `plugins/bob-navigation-hotkeys/manifest.json` from `1.3.0` to `1.4.0`;
- update the `bob-navigation-hotkeys` row in the repo `README.md` to mention the visible dependency block links.

No config migration is required.

## Edge cases

- Parent bullet already has unrelated child bullets: dependency link is inserted at the top of the child block, or after
  existing dependency link bullets.
- Parent bullet already has `**DEPENDENCIES:** [[#^same-id]]`: do not duplicate it.
- Parent bullet has multiple dependencies: create one managed child bullet per dependency.
- Dependency value and block ID differ: keep `[dependsOn:: <id-field>]` and link to `[[#^<actual-block-id>]]`.
- Target task changed while the picker was open: preserve the current stale-line guard and do not add either field or
  navigation bullet.
- Current bullet changed while the picker was open: preserve existing current-line guard behavior where available; if
  the dependency field cannot be written, do not insert the navigation child.
- Existing child blocks may use tabs. The planner should prefer an existing direct-child indentation when present, while
  defaulting to two spaces for a top-level bullet to match the requested shape.

## Validation

Run from the `bob-plugins` workspace:

- `node --check plugins/bob-navigation-hotkeys/main.js`
- ad-hoc Node helper checks for the new exported pure helpers:
  - top-level parent gets `  - **DEPENDENCIES:** [[#^id]]`;
  - existing dependency bullet dedupes;
  - multiple dependency bullets append after the last dependency bullet;
  - unrelated child bullets remain below the dependency group;
  - existing `[id::]` / `^block` mismatch produces dependency value and link block ID separately.
- `npm run validate`
- `git diff --check`

Deployment should be dry-run first with `bob plugins sync` from the `bob-plugins` workspace. Only run the actual sync if
the preview does not show unrelated vault-only changes being removed.
