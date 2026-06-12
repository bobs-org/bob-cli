---
create_time: 2026-06-12 15:21:40
status: done
prompt: sdd/prompts/202606/ctrl6_block_id_keymap.md
---
# Plan: Add `<Ctrl+6>` Selected Block ID Rename/Add Keymap

## Goal

Add a default Obsidian command hotkey to the live `block-id-prompt` plugin so `<Ctrl+6>` opens the block-ID modal for
the block at the active editor selection/cursor. If the current line contains one or more transcluded block links, the
command should prefer renaming the transcluded target block link over naming the local line/block that contains the
embed.

The new command should handle two workflows:

1. Rename an existing block ID on the selected/current block.
2. Add a new block ID to the selected/current block when it does not already have one.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault/plugin workflow context before planning a keymap change for the block-id-prompt plugin"`.
- Vault instructions at `/home/bryan/bob/AGENTS.md`.
- Existing plugin source: `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`.
- Existing plugin manifest: `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`.
- Current hotkey config: `/home/bryan/bob/.obsidian/hotkeys.json`.
- Prior SDD plans:
  - `sdd/tales/202606/obsidian_alias_block_completion_cursor.md`
  - `sdd/tales/202606/obsidian_file_link_caret_jump.md`
  - `sdd/tales/202606/block_id_rename_cache_only.md`
  - `sdd/tales/202606/block_id_popup_preview.md`
  - `sdd/tales/202606/transcluded_ctrl_enter_tasks.md`

No `bob-cli` CLI subcommand or option is involved, so `memory/long/cli_rules.md` is not required.

## Current Implementation Facts

- The relevant code is the live vault plugin at: `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`.
- The plugin is plain JavaScript; `main.js` is the source of truth and there is no build step.
- The plugin currently has no Obsidian command registered. It reacts to typed `^` markers through a CodeMirror
  `EditorView.updateListener`.
- Existing link-triggered rename flow:
  - parses a source wikilink carrying an existing block ID;
  - opens `BlockIdPromptModal`;
  - validates the destination note;
  - checks duplicate/new/old ID constraints;
  - rewrites candidate references from Obsidian metadata cache plus the active/destination notes;
  - renames the destination block token;
  - restores the source link on cancel.
- The modal now has a preview helper, but it assumes an existing `source.oldId` when it reads destination block text.
- `collectWikiBlockReferences()` already finds `[[...#^id...]]` substrings inside transcluded links because the regex
  begins at `[[`; replacing only that span preserves a leading `!`.
- `.obsidian/hotkeys.json` currently has no `Ctrl+6` binding. The plan should add a default command hotkey in the
  plugin, not edit the user hotkey JSON.
- The vault currently has unrelated dirty files. The target plugin file is clean. Later implementation must stage and
  commit only task-related vault changes.

## Product Decisions

1. **Implement this in `block-id-prompt`.**
   - This plugin already owns block-ID rename/add semantics and link-reference rewrites.
   - No `bob-navigation-hotkeys`, `.obsidian.vimrc`, or `hotkeys.json` change is expected.

2. **Register an Obsidian editor command with a default `<Ctrl+6>` hotkey.**
   - Command ID: use a specific plugin-local name such as `rename-selected-block-id`.
   - Command name: use user-facing text such as `Rename selected block ID`.
   - Hotkey: `hotkeys: [{ modifiers: ["Ctrl"], key: "6" }]`.
   - Use `editorCallback` so the command receives the active Markdown editor/view directly.

3. **Transcluded block links on the current line take priority.**
   - Recognize only embedded wikilinks with a leading `!` and a block target, such as:
     - `![[note#^id]]`
     - `![[note.md#^id|Alias]]`
     - `![[folder/note#^id]]`
     - `![[#^id]]`
   - If exactly one supported transcluded block link is on the current line, target that link.
   - If multiple supported transcluded block links are on the current line, prefer the one containing the cursor; if
     none contains the cursor, use the nearest candidate by cursor distance or no-op with a notice if implementation
     shows nearest selection would be ambiguous.
   - For a transcluded link target, reuse the existing link-source rename path. Set `source.startCh`/`source.endCh` to
     the `[[...]]` span, not the leading `!`, so reference replacement preserves transclusion.

4. **Only fall back to the local selected/current block when no transcluded block link is selected.**
   - Interpret "selected block" as the Markdown block containing the single active cursor or selection head.
   - If a non-empty selection spans lines, accept it only when it resolves to one Markdown block; otherwise show a short
     notice and do nothing rather than guessing.
   - Do nothing inside fenced code blocks, matching the existing typed-marker path.

5. **Support local rename and local add as first-class source modes.**
   - Existing link-triggered sources keep current behavior.
   - Add a direct/local source mode for current-file block operations:
     - existing local ID: rename that ID and update references to the current file's block;
     - missing local ID: add `^new-id` to the selected/current block and do not attempt reference rewrites.
   - Keep duplicate checks against the current/destination note before adding or renaming.

6. **Keep existing link-triggered behavior stable.**
   - Do not change the typed `^`/`^^` marker flow, file-link caret jump, cancel behavior, backlink rewrite collection,
     duplicate validation, or preview extraction except where direct/local sources need a small extension.

## Implementation Scope

Expected vault file to edit after this plan is approved:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`

No expected edits to:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/community-plugins.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/*`
- Any `bob-cli` Rust source, scripts, README, fixtures, or CLI docs

## Implementation Plan

1. **Add the command registration in `onload()`.**
   - Register `rename-selected-block-id` with
     `editorCallback: (editor, view) => this.openSelectedBlockIdPrompt(editor, view)`.
   - Keep the existing CodeMirror update listener registration intact.

2. **Add transcluded block-link detection for the active line.**
   - Add a small helper near existing wikilink parsers, for example
     `findTranscludedBlockReferenceOnLine(lineText, cursorCh)`.
   - Reuse `WIKI_LINK_RE`, `splitWikiLinkBody()`, and `parseBlockReferenceDestination()` so supported block-link grammar
     matches the existing rename/rewrite code.
   - Require `lineText[match.index - 1] === "!"`.
   - Return a source-compatible object with `raw`, `targetText`, `oldId`, `aliasSuffix`, `blockPrefix`, `startCh`,
     `endCh`, and a source mode like `kind: "link-block"`.

3. **Add selected/current local block discovery.**
   - Build from `editor.getCursor()`, `editor.listSelections()` when available, `editor.getLine()`, and
     `editor.getValue()`.
   - Require one selection/cursor, a Markdown file, and a nonblank target line/block.
   - Identify the block range conservatively:
     - for a single-line task/list/heading/paragraph line, use that line;
     - for contiguous nonblank paragraphs/blockquote/callout/table blocks, expand to adjacent nonblank lines;
     - if the cursor is on a standalone `^id` line, attach it to the previous contiguous block;
     - if the next line is a standalone `^id` for the current block, include it as the existing ID.
   - Detect existing block IDs in the selected block range using `blockTokenMatches()` plus line/range checks.
   - If exactly one block ID belongs to the range, produce a direct rename source with `oldId` and exact token position.
   - If no block ID belongs to the range, produce a direct add source with insertion position and preview text.
   - If multiple block IDs belong to the range, show a notice and do nothing.

4. **Extend the modal to support direct sources without disturbing link sources.**
   - For direct sources, pass a `previewText` computed from the selected block so add mode can still show useful
     context.
   - For direct rename sources, prefill the input with the current ID and select the text on focus.
   - For direct add sources, keep the input blank with the existing placeholder.
   - Leave existing typed-link modal behavior unchanged unless a small shared option is needed.

5. **Generalize submit handling by source mode.**
   - Keep `submitBlockId(source, newId)` as the modal entry point, but branch internally:
     - link source: current behavior;
     - direct rename source: validate the active/destination content, build a reference rewrite plan without requiring
       the source marker link to be found, apply reference rewrites, then rename the destination token;
     - direct add source: validate duplicate absence and insert `^newId` at the recorded insertion point.
   - Refactor only as much as needed. A helper such as `submitLinkedBlockId()`, `submitDirectBlockRename()`, and
     `submitDirectBlockAdd()` is preferable to making the existing method harder to follow.

6. **Adjust reference planning for direct rename.**
   - The current `buildReferenceRewritePlan()` requires the initiating source marker link to be found. That remains
     correct for link-triggered sources.
   - Add an option or source-mode branch that skips only that "source marker planned" requirement for direct local
     renames.
   - Continue using the same candidate files, destination resolution, raw-link verification, non-overlap validation,
     unchanged-before-write checks, and unsupported Markdown-link blocking.

7. **Add direct block insertion/rename helpers.**
   - Existing direct rename can reuse `renameDestinationBlock()` if the old ID is unique and the destination is the
     active file; otherwise add a small active-editor rewrite helper that verifies the stored token still exists.
   - Direct add should:
     - re-read current active editor content;
     - verify the selected block/insertion anchor has not changed materially since the modal opened;
     - append ` ^newId` to single-line blocks;
     - for multi-line blocks where appending would be ambiguous, insert a standalone `^newId` line immediately after the
       selected block;
     - call `suppressEditorScans()` around editor mutations.

8. **Notices and completion messages.**
   - Preserve current validation notices for blank IDs, invalid characters, duplicate IDs, unresolved targets, and stale
     content.
   - Add concise notices for:
     - no active Markdown block selected;
     - multiple block IDs in the selected block;
     - ambiguous multiple transcluded block links if the implementation chooses not to pick nearest;
     - successful add, such as `Added block ID`.
   - Keep existing rename success messages for link-triggered and direct rename flows, with `editCount` accurate even
     when no references were rewritten.

## Acceptance Criteria

- Pressing `<Ctrl+6>` on a normal task line with no block ID opens the modal; saving `foo` changes:
  `- [ ] #task Pay invoice` to: `- [ ] #task Pay invoice ^foo`.
- Pressing `<Ctrl+6>` on `- [ ] #task Pay invoice ^foo` opens the modal prefilled with `foo`; saving `bar` renames the
  block token to `^bar` and rewrites valid wiki references to that block.
- Pressing `<Ctrl+6>` on a paragraph or blockquote with a standalone following `^foo` line renames the standalone ID.
- Pressing `<Ctrl+6>` on a paragraph/blockquote with no ID adds a usable block ID without corrupting the block text.
- Pressing `<Ctrl+6>` on a line containing `![[bob#^foo]]` targets the transcluded `foo` block, not the local line that
  contains the embed.
- A transcluded target rename preserves the leading `!` in the source embed and uses the existing backlink rewrite
  behavior.
- Multiple transcluded block links on one line are handled deterministically by cursor containment/nearest selection or
  blocked with a clear notice.
- Existing typed `^` behavior after wikilinks still works, including file-link cursor jump and block-link rename/cancel.
- The command does nothing inside fenced code blocks.
- `.obsidian/hotkeys.json` is not modified.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/block-id-prompt/main.js
git -C /home/bryan/bob status --short
git status --short
```

Focused Node checks with a VM/stubbed Obsidian environment:

- Transcluded parser recognizes `![[bob#^id]]`, `![[bob.md#^id|Alias]]`, `![[folder/note#^id]]`, and `![[#^id]]`.
- Transcluded parser ignores plain `[[bob#^id]]`, non-block embeds, headings, and malformed IDs.
- Candidate selection chooses a cursor-contained transcluded block link when multiple are present.
- Direct block discovery finds:
  - single-line task ID;
  - single-line task without ID;
  - standalone following ID;
  - cursor on standalone ID line;
  - multi-line paragraph/blockquote without ID;
  - multiple-ID ambiguity.
- Direct add helper appends/inserts the expected `^new-id` text and preserves line endings.
- Direct rename path skips only the link-source-marker requirement while preserving duplicate and exactly-once old-ID
  validation.
- Existing typed-marker parser cases from prior work still pass.

Manual live-vault smoke checks after reloading the plugin:

1. In a scratch note, press `<Ctrl+6>` on a task without a block ID, add an ID, and confirm the preview/input behavior.
2. Press `<Ctrl+6>` on that same task, rename the ID, and confirm valid references update.
3. Repeat on a paragraph or blockquote with a standalone `^id`.
4. Repeat on a line containing a transcluded embed such as `- ![[bob#^some-test-id]]`; confirm the transcluded source
   block is renamed and the embed keeps `!`.
5. Confirm duplicate ID attempts still block.
6. Confirm cancel leaves the local block or transcluded link unchanged.
7. Confirm the existing typed `^` after a file link and after a block link still behaves as before.

Before terminating after implementation, follow `/home/bryan/bob/AGENTS.md`:

- inspect vault status;
- stage only task-related vault file changes;
- commit those vault changes through the required `/sase_git_commit` workflow;
- leave unrelated dirty vault files untouched.

## Risks and Mitigations

- **Markdown block boundaries are broader than this plugin currently parses.** Keep local block detection conservative,
  use clear notices for ambiguity, and avoid changing content when selection/range ownership is unclear.
- **Direct rename has no initiating source link.** Reuse existing reference rewriting but explicitly skip only the
  source-marker requirement for this mode; preserve all duplicate, resolution, raw-span, and unchanged-file checks.
- **Adding IDs to multi-line blocks can be surprising.** Prefer inline append for simple single-line blocks and a
  standalone ID line for multi-line blocks, which matches the preview helper's existing standalone-ID support.
- **Active editor content can change while the modal is open.** Store enough expected block/token context and revalidate
  immediately before mutation.
- **Vault sync/unrelated dirty files.** Re-check status before editing and before committing; commit only
  `.obsidian/plugins/block-id-prompt/main.js` if that is the only task-related vault file changed.
