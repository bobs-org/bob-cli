---
create_time: 2026-06-12 15:10:47
status: done
prompt: sdd/prompts/202606/block_id_popup_preview.md
---
# Plan: Show Block Contents in the Block ID Rename Popup

## Goal

When the `block-id-prompt` Obsidian plugin opens its `^`-triggered block ID rename modal, show the contents of the block
being renamed above the ID input. The preview should help the user remember what task or block the existing ID belongs
to without changing the current rename, cancel, validation, or backlink rewrite behavior.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian workflow context before planning block-link rename popup changes"`.
- Vault instructions at `/home/bryan/bob/AGENTS.md`.
- Current plugin source: `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`.
- Prior related SDD notes:
  - `sdd/tales/202606/block_id_rename_cache_only.md`
  - `sdd/tales/202606/obsidian_file_link_caret_jump.md`

## Current Implementation Facts

- The relevant implementation is the live vault plugin at `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`,
  not Rust code in `bob-cli`.
- The plugin is plain JavaScript with no build step. `main.js` is the source of truth.
- `inspectActiveEditor()` parses the marker, builds a `source` object, and opens `BlockIdPromptModal`.
- `source` already contains the old block ID, the link target text, the source editor, source path, and source line.
- `submitBlockId()` already resolves and reads the destination note through `readDestinationForValidation(source)`. That
  helper reads from the live editor buffer when the destination is the active note.
- The modal currently renders only:
  - `h2` title: `Block ID`
  - a `Setting` named `ID` with the text input
  - Cancel and Save buttons
- The vault currently has unrelated uncommitted changes, but no existing diff in
  `.obsidian/plugins/block-id-prompt/main.js`.

## Product Behavior

1. Show a read-only preview above the ID input whenever the rename popup opens.
2. Prefer the actual block text over the link text.
3. Keep the popup fast and non-blocking:
   - Render the modal immediately.
   - Create the preview element before the input setting so it is physically above the input.
   - Fill it asynchronously after resolving and reading the destination.
4. Do not use the preview load as validation authority. Submit-time validation remains the source of truth.
5. Do not emit Obsidian notices just because preview loading fails. The rename path already reports real validation
   failures on submit.

## Implementation Plan

Expected file to edit after this plan is submitted:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`

No expected edits to:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`
- Any `bob-cli` Rust source, docs, fixtures, or CLI command definitions

Planned changes:

1. Add a small pure helper near the existing text helpers, tentatively `extractBlockPreviewText(content, id)`.
   - Use existing `blockTokenMatches(content, id)` so the preview identifies the same `^old-id` token shape as the
     existing rename validator.
   - If exactly one match exists and the ID appears on a content line, return that line with the `^old-id` token
     removed.
   - If the ID is on a standalone line, return the contiguous non-blank block immediately above it. This covers common
     Obsidian block forms such as blockquotes or paragraphs followed by a standalone block ID line.
   - Preserve useful Markdown markers like task checkboxes, list bullets, links, and properties because they provide
     context.
   - Return `null` when no unique preview can be found.

2. Add a plugin method, tentatively `readBlockPreviewText(source)`.
   - Reuse `readDestinationForValidation(source)` to resolve the same destination file the rename flow will validate.
   - Return the extracted preview text when the destination is readable.
   - Return `null` on unresolved, unreadable, missing, or non-unique blocks.
   - Avoid notices in this method; log only if the reused read helper already logs.

3. Extend `BlockIdPromptModal`.
   - Add a preview container before the `new Setting(...).setName("ID").addText(...)` call.
   - Style it inline with Obsidian theme variables because this plugin has no `styles.css` today and the change should
     remain single-file:
     - secondary background
     - subtle border
     - normal text color
     - `white-space: pre-wrap`
     - wrapping for long links or task text
     - bounded max height with vertical scrolling for multi-line blocks
   - Set an accessibility label such as `Current block contents`.
   - Start with a neutral loading state, update to the block text if available, and fall back to a short unavailable
     state if extraction fails.
   - Keep input focus behavior unchanged.

4. Preserve the existing rename mechanics.
   - Do not change marker parsing, source replacement, duplicate checks, candidate reference collection, backlink
     rewrite planning, cancel behavior, or the file-link jump behavior.
   - Do not add a new setting or command.

## Acceptance Criteria

- For a task line like `- [ ] #task Pay invoice [p::2] ^bill`, opening the rename popup for `^bill` shows
  `- [ ] #task Pay invoice [p::2]` above the input.
- For a blockquote or paragraph with a standalone following ID, the popup shows the preceding block content rather than
  a blank preview.
- The ID input still autofocuses and Enter still submits.
- Cancel still restores the source link exactly as before.
- Save still performs the same duplicate checks, link rewrites, destination rename, and notices as before.
- If the target note cannot be resolved or the old ID is not uniquely found, the popup still opens and the submit path
  still reports the existing validation failure.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/block-id-prompt/main.js
git -C /home/bryan/bob status --short
git status --short
```

Focused behavior checks:

- Use a small temporary note in the live vault with a task carrying a block ID, link to it, press `^` after the block
  link, and confirm the modal shows the task text above the input.
- Repeat with a paragraph or blockquote followed by a standalone `^id` line.
- Rename to a fresh ID and confirm the block ID and backlink update.
- Rename to a duplicate ID and confirm the existing duplicate guard still blocks the change.
- Open the modal and cancel, then confirm the source link is restored.

## Risks and Mitigations

- Obsidian block parsing is broader than this plugin's current needs. The preview will use a conservative heuristic:
  exact ID token, same-line text first, previous contiguous block for standalone IDs. Submit validation remains exact.
- Asynchronous preview loading could update after the modal closes. The modal method should check that the modal is
  still open before mutating the preview element.
- The vault is actively synced and already dirty. Implementation must inspect status before editing, change only
  `.obsidian/plugins/block-id-prompt/main.js`, avoid unrelated files, and follow the vault instruction to commit
  task-related vault changes with the SASE git commit workflow before terminating after edits.
