---
create_time: 2026-06-14 11:28:49
status: done
prompt: sdd/prompts/202606/task_dependency_block_ids.md
---
# Use Block IDs for Generated Task Dependency IDs

## Goal

When Tasks auto-suggest inserts a `[dependsOn:: <id>]` dependency and backfills `[id:: <id>]` on the target task, use
the target task's existing Obsidian block ID as `<id>` when possible. Do not patch
`.obsidian/plugins/obsidian-tasks-plugin/main.js`; the solution must live in vault-synced custom automation so another
synced machine gets the behavior.

## Current Behavior

The installed Tasks plugin generates a missing task ID through a private helper in its bundled `main.js`:

- If the target task already has an `id`, Tasks reuses it.
- If the target task has no `id`, Tasks generates a random six-character base-36 ID.
- The target task line is rewritten with `[id:: <random>]`.
- The active task gets `[dependsOn:: <random>]`.

The helper is private inside the Tasks bundle, so changing it directly would be a local plugin patch and would not
satisfy the reproducibility requirement.

## Proposed Approach

Add the behavior to the vault-synced custom task plugin, preferably
`~/bob/.obsidian/plugins/task-status-cycler/main.js`, since it already owns Bryan-specific task-line mutations and is
synced with the vault.

Implement a normalization pass that runs after Tasks writes dependency IDs:

1. Detect task lines containing both an inline `[id:: <generated>]` field and a trailing Obsidian block ID `^block-id`.
2. If the inline ID looks like a Tasks-generated ID and differs from the block ID, rewrite the task line to
   `[id:: block-id]`.
3. Rewrite matching `[dependsOn:: <generated>]` values to `[dependsOn:: block-id]`.
4. Preserve user-authored IDs by only rewriting IDs that match Tasks' generated shape and were observed together with a
   block ID.

Run this in two places:

- Active editor debounce: handles the common same-file flow immediately after Tasks inserts the dependency.
- Vault-file debounce from metadata/file modification events: handles cross-file dependency creation where Tasks edits a
  target file outside the active editor.

For cross-file rewrites, collect a short-lived mapping of `generatedId -> blockId` from rewritten target task lines,
then apply it to affected Markdown files. Prefer a narrow pass over open/current files first; if needed, scan vault
Markdown files for exact `[dependsOn:: generatedId]` occurrences.

## Safety Rules

- Never modify the stock `obsidian-tasks-plugin` files.
- Do not overwrite an existing meaningful `[id:: ...]` that is not a Tasks-generated six-character ID.
- Do not add or change block IDs.
- Preserve trailing block ID placement as the final token.
- Support comma-separated `dependsOn` values and whitespace variations.
- Avoid loops by skipping writes when normalized content is identical.

## Verification

Add small helper-level tests if we keep logic factored inside `task-status-cycler/main.js` and expose helpers under the
existing `module.exports.helpers` pattern if present or add one. Cover:

- Target line with `[id:: abc123] ^real-id` rewrites to `[id:: real-id] ^real-id`.
- Source line `[dependsOn:: abc123]` rewrites to `[dependsOn:: real-id]`.
- Comma-separated dependency lists preserve unrelated IDs.
- Existing `[id:: custom-id] ^real-id` is not rewritten when it does not match the generated-ID heuristic.
- Lines without block IDs are unchanged.

Then validate manually in a scratch vault note:

1. Create a target task with a block ID and no `[id:: ...]`.
2. Use Tasks autocomplete to add `[dependsOn:: ...]` from another task.
3. Confirm the target gets `[id:: <block-id>]` and the source gets `[dependsOn:: <block-id>]`.
4. Repeat with a target task that already has `[id:: custom]`; confirm Tasks/custom behavior is preserved.

## Open Question

The generated-ID heuristic should probably start as `^[0-9a-z]{6}$`, matching Tasks 8.0.0's current helper. If Tasks
changes this upstream, the normalizer will safely stop rewriting rather than risk touching intentional IDs.
