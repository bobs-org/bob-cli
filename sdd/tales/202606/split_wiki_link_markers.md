---
create_time: 2026-06-19 21:15:16
status: wip
prompt: sdd/prompts/202606/split_wiki_link_markers.md
---
# Split Obsidian Wiki Link `^` and `@` Trigger Behavior

## Goal

Change the live Bob vault `block-id-prompt` plugin so the character typed immediately after the final `]` of an Obsidian
wikilink has one stable meaning:

- `^` always performs the file-link block-completion preparation path: consume the transient trailing marker, remove any
  alias suffix, insert `^` immediately before the first closing `]`, and leave the cursor after that inserted caret so
  Obsidian's normal block-link completion menu can open.
- `@` performs the block-ID rename prompt path for links that already target a block ID.

The current mixed trailing-`^` behavior should be removed: a trailing `^` after an existing block link should no longer
open the block-ID rename modal.

## Context Reviewed

- Used the `sase_plan` skill and will submit this plan with `sase plan propose`.
- Used the `sase_memory_read` skill as required for Obsidian-domain work:
  `sase memory read obsidian.md --reason "Need Obsidian vault/link workflow context before changing Obsidian link caret behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; edits under `~/bob` require checking vault status first, preserving unrelated dirty
  work, and committing task-related vault changes with the SASE git commit workflow before termination.
- Inspected `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`, which owns this behavior.
- Reviewed prior relevant plans:
  - `sdd/tales/202606/obsidian_alias_block_completion_cursor.md`
  - `sdd/tales/202606/obsidian_file_link_caret_jump.md`
  - `sdd/tales/202606/auto_block_link_completion.md`
- Checked `git -C /home/bryan/bob status --short`; the vault is already dirty, including pre-existing modifications to
  `.obsidian/plugins/block-id-prompt/main.js`, `.obsidian/plugins/bob-navigation-hotkeys/main.js`,
  `.obsidian/plugins/task-status-cycler/main.js`, and many notes. The target plugin diff appears to be the recent
  auto-block-completion work. Implementation must work with that diff and must not revert or stage unrelated changes.
- This task does not add or change `bob` CLI subcommands or options, so `memory/cli_rules.md` is not triggered.

## Current Implementation Facts

- `block-id-prompt/main.js` is direct plugin source; there is no build step for this plugin.
- The plugin watches CodeMirror document changes through `EditorView.updateListener`, debounces scans, and inspects the
  active Markdown editor.
- `inspectActiveEditor()` requires an active Markdown view, a single cursor, an active Markdown file, mutation-capable
  editor APIs, and a cursor line outside fenced code blocks.
- `findMarkerLinkNearCursor()` scans wikilinks on the current line and chooses the nearest parsed marker candidate.
- Current trailing marker parsing is mixed:
  - `parseTrailingMarkerLink()` consumes a single trailing `^` after wikilinks whose destination already ends in a block
    reference, then opens the block-ID rename modal.
  - `parseTrailingFileLinkJumpMarker()` consumes a single trailing `^` after non-block wikilinks, removes aliases, and
    uses the current uncommitted auto-completion path to produce `[[destination^]]`.
- The current uncommitted auto-completion path has two useful pieces that should be preserved:
  - `dispatchFileLinkBlockCompletion()` performs a single CM6 transaction with `userEvent: "input.type"`.
  - `applyFileLinkBlockCompletionWithEditorApi()` falls back through Obsidian's editor abstraction.
- Existing inline `#^^old-id` marker behavior is separate from the trailing-character behavior because the extra marker
  is inside the link target, not after the final `]`.

## Product Decisions

1. Keep the change in `block-id-prompt`.
   - This plugin already owns the trailing wikilink marker workflow.
   - No hotkey, vimrc, Rust CLI, or separate Obsidian plugin change is needed.

2. Make trailing `^` unconditional for completion.
   - If the user types `^` after `[[target]]`, `[[target|alias]]`, or `[[target#^old-id|alias]]`, the plugin should take
     the completion path.
   - Preserve the destination text exactly as written, except for deleting the alias suffix starting at the first `|`.
   - For an existing block link, this means `[[target#^old-id|alias]]^` becomes `[[target#^old-id^]]` with the cursor
     after the inserted caret. This follows the requested "second behavior" mechanically and avoids opening rename.

3. Make trailing `@` the rename trigger for block links.
   - `[[target#^old-id|alias]]@` should open the same modal path that trailing `^` currently opens.
   - The transient `@` should be part of the source span so cancel/save removes it exactly like the old trailing `^`
     marker was removed.
   - `@` after a non-block link should not be consumed, because there is no existing block ID to rename.

4. Preserve existing inline marker behavior unless implementation proves it conflicts.
   - `[[target#^^old-id|alias]]` can keep opening the rename modal.
   - For a literal trailing `^`, the trailing caret completion parser should win over inline parsing when both are
     present, so "typed after the final `]`" remains stable.

5. Keep existing safety gates.
   - Do nothing for multiple cursors, non-Markdown files, stale line slices, or fenced code blocks.
   - Preserve the double-marker guard (`^^` / `@@`) so accidental repeated markers are not consumed.
   - Suppress the plugin's own follow-up scans around automated edits.
   - Do not show a notice for the normal completion path.

## Implementation Scope

Expected implementation file:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`

No expected edits:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/community-plugins.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- Any `bob-cli` Rust source, scripts, README, or fixtures

## Proposed Technical Design

1. Add or reuse a tiny trailing-marker helper.
   - Example: `hasSingleTrailingMarker(lineText, markerCh, markerChar)`.
   - It should require `lineText[markerCh] === markerChar` and `lineText[markerCh + 1] !== markerChar`.

2. Replace the old trailing-`^` block rename parser with a trailing-`@` parser.
   - Rename `parseTrailingMarkerLink()` to something like `parseTrailingAtBlockRenameMarker()`.
   - Look for `@`, not `^`, after the matched wikilink.
   - Split the link body with `splitWikiLinkBody()`.
   - Reuse `parseTrailingBlockDestination(destination)` to preserve current block-target parsing, including supported
     `#^old-id` and bare `^old-id` forms.
   - Return a source object with `raw: raw + "@"`, parsed `targetText`, `oldId`, `blockPrefix`, `aliasSuffix`,
     `startCh`, and `endCh`.

3. Change the caret completion parser to apply to every non-empty wikilink destination.
   - Rename `parseTrailingFileLinkJumpMarker()` if useful, for example to `parseTrailingCaretCompletionMarker()`.
   - Keep its single trailing `^` guard.
   - Keep `normalizeText(destination)` only as an emptiness guard.
   - Remove the current `parseBlockReferenceDestination(...){ return null }` exclusion so block links follow the
     completion path too.
   - Preserve the current uncommitted auto-completion output fields:
     - `plainReplacement: [[${destination}]]`
     - `completionReplacement: [[${destination}^]]`
     - `insertionCh: match.index + 2 + destination.length`
     - `finalCursorCh: insertionCh + 1`

4. Adjust marker parse order to make the requested trailing-character semantics explicit.
   - Try trailing caret completion first.
   - Try trailing at-sign block rename second.
   - Try inline `#^^old-id` rename after those trailing marker checks.
   - This keeps a typed trailing `^` from being stolen by the inline rename parser while leaving ordinary inline markers
     functional.

5. Keep the editor action split already present in `inspectActiveEditor()`.
   - `kind: "file-link-jump"` continues to call `applyFileLinkJumpMarker()`, which should keep using the CM6 dispatch
     path plus editor API fallback from the current working tree.
   - All other parsed marker sources continue through `openBlockIdPrompt(source)`.
   - No modal, rename, reference-rewrite, or candidate-file logic should change.

6. Preserve dirty-work discipline.
   - Re-check `git -C /home/bryan/bob status --short` immediately before editing.
   - Patch only `.obsidian/plugins/block-id-prompt/main.js`.
   - Do not revert the pre-existing auto-completion diff or unrelated vault changes.
   - After implementation and verification, use the required SASE git commit workflow for task-related vault changes.

## Acceptance Criteria

- `[[path/to/foobar]]^` becomes `[[path/to/foobar^]]`, with the cursor after the inserted `^`.
- `[[path/to/foobar|foobar]]^` becomes `[[path/to/foobar^]]`, with the alias removed and cursor after the inserted `^`.
- `[[path/to/foobar#^old-id|alias]]^` becomes `[[path/to/foobar#^old-id^]]` and does not open the block-ID rename modal.
- `[[path/to/foobar#^old-id|alias]]@` opens the existing block-ID rename modal for `old-id`.
- Canceling the modal from the trailing-`@` path removes the transient `@` and restores the original link text, matching
  the old trailing-`^` cancel semantics.
- Saving the modal from the trailing-`@` path rewrites the source link and block references exactly as the old
  trailing-`^` rename path did.
- `[[path/to/foobar]]@` is not consumed because the link has no old block ID to rename.
- `[[path/to/foobar]]^^` and `[[path/to/foobar#^old-id]]@@` are not consumed.
- No prompt or completion action runs inside fenced code blocks.
- Multiple wikilinks on one line still choose the nearest marker around the cursor.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/block-id-prompt/main.js
```

Focused Node behavior harness with stubbed `obsidian` and `@codemirror/view` modules:

- Instantiate the plugin with fake `MarkdownView`, `TFile`, and editor objects.
- For `[[path/to/foobar|alias]]^`, assert the editor fallback produces `[[path/to/foobar^]]` and no modal source is
  captured.
- For `[[path/to/foobar#^old-id|alias]]^`, assert the editor fallback produces `[[path/to/foobar#^old-id^]]` and no
  modal source is captured.
- For `[[path/to/foobar#^old-id|alias]]@`, monkeypatch `openBlockIdPrompt()` and assert it receives `oldId: "old-id"`,
  `blockPrefix: "#^"`, `aliasSuffix: "|alias"`, and `raw` ending in `@`.
- For `[[path/to/foobar]]@`, assert no edit and no prompt.
- For double-marker guards, assert `^^` and `@@` are ignored.

Manual live-vault acceptance after reloading Obsidian or the plugin:

1. Press `^` after a plain wikilink and confirm the block completion menu opens from inside the link.
2. Press `^` after an aliased wikilink and confirm the alias is removed before the menu opens.
3. Press `^` after an existing block wikilink and confirm no rename modal opens.
4. Press `@` after an existing block wikilink and confirm the rename modal opens and save/cancel behave as before.
