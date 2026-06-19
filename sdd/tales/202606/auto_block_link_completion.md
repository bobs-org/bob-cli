---
create_time: 2026-06-19 08:44:21
status: wip
prompt: sdd/prompts/202606/auto_block_link_completion.md
---
# Auto Block Link Completion Plan

## Goal

Make the existing `block-id-prompt` trailing-`^` workflow complete the next step automatically.

Today, for a non-block Obsidian wikilink, typing `^` immediately after the final `]` consumes that transient marker,
removes any alias suffix, and moves the cursor inside the link just before the closing `]]`. The user then types `^`
again to invoke Obsidian's built-in block-link completion menu.

The new behavior should make the first trailing `^` produce the same final editor state as the manual two-step flow:

```markdown
[[path/to/foobar|foobar]]^
```

should become:

```markdown
[[path/to/foobar^]]
```

with the cursor immediately after the inserted `^`, so Obsidian's normal block completion menu opens for
`path/to/foobar`.

For non-aliased file links:

```markdown
[[path/to/foobar]]^
```

should also become:

```markdown
[[path/to/foobar^]]
```

Block links must keep the existing block-ID rename behavior:

```markdown
[[path/to/foobar#^old-id|alias]]^
```

should still open the `block-id-prompt` rename modal, not auto-trigger block completion.

## Context Reviewed

- Read `sase_plan` skill instructions and will submit this file with `sase plan propose`.
- Read `sase_memory_read` skill instructions.
- Read Obsidian memory through:
  `sase memory read obsidian.md --reason "Need Obsidian vault and link workflow context before planning changes to Obsidian link completion behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect vault status first, preserve unrelated changes, and commit
  only task-related vault files if implementation changes files under `~/bob`.
- Inspected `git -C /home/bryan/bob status --short`; the vault currently has pre-existing unrelated modifications, but
  `.obsidian/plugins/block-id-prompt/main.js` is not currently listed as modified.
- Inspected the current implementation: `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`.
- Reviewed prior completed plans:
  - `sdd/tales/202606/obsidian_alias_block_completion_cursor.md`
  - `sdd/tales/202606/obsidian_file_link_caret_jump.md`
- Checked local custom plugins for existing suggestion-trigger patterns; no custom plugin currently calls a public
  Obsidian API to open the built-in block-link suggester.
- Checked official editor references:
  - Obsidian exposes `Editor.replaceRange()` for active Markdown source edits and exposes the CodeMirror editor behind
    the editor abstraction: https://docs.obsidian.md/Plugins/Editor/Editor
  - CodeMirror transactions support `changes`, `selection`, `scrollIntoView`, and a `userEvent` shorthand; core typed
    input uses `input.type`: https://codemirror.net/docs/ref/
- This task does not add or change `bob` CLI subcommands or options, so the `memory/long/cli_rules.md` trigger does not
  apply.

## Current Implementation Facts

- `block-id-prompt` registers a CodeMirror `EditorView.updateListener` and debounces document scans with
  `SCAN_DEBOUNCE_MS`.
- `inspectActiveEditor()` requires:
  - active Markdown view,
  - single cursor,
  - active Markdown file,
  - editor mutation APIs,
  - cursor line outside fenced code blocks.
- `findMarkerLinkNearCursor()` scans wikilinks on the current line and chooses the nearest parsed marker candidate.
- Marker parse order is important:
  1. `parseInlineMarkerLink()` handles inline `#^^old-id`.
  2. `parseTrailingMarkerLink()` handles trailing `^` after links that already target blocks.
  3. `parseTrailingFileLinkJumpMarker()` handles trailing `^` after non-block file wikilinks.
- The current file-link branch returns `kind: "file-link-jump"`, `replacement: [[destination]]`, and `cursorCh` just
  before the closing brackets.
- `applyFileLinkJumpMarker()` verifies the source slice, calls `suppressEditorScans()`, replaces the marker span, sets
  the cursor to `cursorCh`, and clears `lastPromptKey`.
- Existing helper `setEditorCursorIfPossible()` is editor-abstraction based. Existing nearby custom plugins use CM6
  dispatch where exact selection/scroll behavior matters.

## Product Decisions

1. Keep the feature in `block-id-prompt`.
   - The plugin already owns the trailing-`^` command marker after wikilinks.
   - This is not a CLI, hotkey, vimrc, or separate navigation-plugin change.

2. Do not try to implement a custom block completion menu.
   - The goal is to trigger Obsidian's existing block-link completion UI.
   - The plugin should only create the same document and cursor state that the user's second `^` currently creates.

3. Preserve block rename precedence.
   - Existing block-link marker parsing remains before the file-link branch.
   - Any destination parsed by `parseBlockReferenceDestination(destination, { allowPathBareBlock: true })` remains
     excluded from file-link completion.

4. Make the auto-completion path equivalent to the manual second keypress.
   - For non-block wikilinks, the final text should include an inserted `^` inside the link.
   - The final cursor should be immediately after that inserted `^`.
   - Aliases should still be removed exactly as the current file-link jump does.

5. Prefer a single CM6 transaction when available.
   - A CM6 dispatch can replace the full marker span with `[[destination^]]`, place the cursor after the `^`, mark the
     change as `userEvent: "input.type"`, and scroll the selection into view.
   - This gives Obsidian's editor and suggester the strongest signal that the final `^` is typed input, while keeping
     undo behavior coherent.

6. Keep a conservative editor-API fallback.
   - If CM6 dispatch is unavailable or fails, fall back to the Obsidian editor abstraction:
     1. replace the marker span with `[[destination]]`,
     2. set the cursor to the inside-link insertion point,
     3. insert `^` at that cursor with `editor.replaceRange("^", cursor)`,
     4. set the final cursor after the inserted `^`.
   - If live testing shows the fallback insertion happens too early for the suggester, defer only the final `^`
     insertion by one animation frame.

7. Avoid private Obsidian internals unless the text-insertion path fails in live testing.
   - No DOM scraping of suggestion popups.
   - No synthetic keyboard events as the first implementation path.
   - No dependency on undocumented Obsidian suggester classes.

## Implementation Scope

Expected implementation file after plan approval:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`

No expected edits:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/community-plugins.json`
- `/home/bryan/bob/obsidian_vimrc.md`
- Any `bob-cli` Rust source, scripts, tests, or README

## Proposed Technical Design

1. Extend the file-link marker object returned by `parseTrailingFileLinkJumpMarker()`.
   - Keep existing guards:
     - exactly one trailing `^` after `]]`,
     - no consumption of `^^`,
     - non-empty destination,
     - block destinations return `null`.
   - Add fields that describe both the plain jump and the auto-completion result:
     - `destination`
     - `plainReplacement: [[${destination}]]`
     - `completionReplacement: [[${destination}^]]`
     - `insertionCh: match.index + 2 + destination.length`
     - `finalCursorCh: insertionCh + 1`
   - Preserve destination text exactly as currently written; use trimming only as an emptiness guard.

2. Replace `applyFileLinkJumpMarker()` with an auto-completion action, or keep the name and update its behavior.
   - Keep the line-slice verification: `lineText.slice(marker.startCh, marker.endCh) === marker.raw`.
   - Call `suppressEditorScans()` before mutating.
   - Clear `lastPromptKey` after the mutation attempt.

3. Add a CM6 helper for the preferred path.
   - Example shape: `dispatchFileLinkBlockCompletion(editor, line, marker)`.
   - Feature-detect:
     - `editor.cm`
     - `cm.dispatch`
     - `cm.state.doc.line`
   - Convert line/ch to document offsets from `cm.state.doc.line(line + 1)`.
   - Dispatch:
     - `changes: { from, to, insert: marker.completionReplacement }`
     - `selection: { anchor: lineStart + marker.finalCursorCh }`
     - `userEvent: "input.type"`
     - `scrollIntoView: true`
   - Return `true` only when dispatch succeeds.

4. Add an editor-API fallback helper.
   - Example shape: `applyFileLinkBlockCompletionWithEditorApi(editor, line, marker)`.
   - Replace the marker span with `marker.plainReplacement`.
   - Set cursor to `{ line, ch: marker.insertionCh }`.
   - Insert `^` at that cursor with `editor.replaceRange("^", cursor)`.
   - Set cursor to `{ line, ch: marker.finalCursorCh }`.
   - If direct insertion does not open Obsidian's block completion in manual testing, change only this fallback to defer
     the final insertion with `requestAnimationFrame`.

5. Keep existing block prompt behavior untouched.
   - `parseInlineMarkerLink()`, `parseTrailingMarkerLink()`, `openBlockIdPrompt()`, modal save/cancel behavior, backlink
     rewriting, and direct `Ctrl+6` behavior should not change.

6. Keep scan suppression narrow.
   - The plugin should suppress its own follow-up scans around the automated rewrite/insert.
   - It should not suppress or block Obsidian's built-in completion system.
   - If the CM6 transaction path is used, it should not depend on `scheduleScan()` seeing the inserted `^`.

## Acceptance Criteria

- `[[path/to/foobar]]^` becomes `[[path/to/foobar^]]`, with the cursor after the inserted `^`, and Obsidian's block
  completion menu opens for `path/to/foobar`.
- `[[path/to/foobar|foobar]]^` becomes `[[path/to/foobar^]]`, with the alias removed, cursor after the inserted `^`, and
  Obsidian's block completion menu opens.
- The existing block-ID rename modal still opens for `[[path/to/foobar#^old-id|foobar]]^`.
- Existing inline block marker behavior still works for `[[path/to/foobar#^^old-id|foobar]]`.
- Existing supported bare block-target behavior still works for `[[path/to/foobar^old-id|foobar]]^`.
- `[[path/to/foobar]]^^` is not consumed by the file-link completion path.
- No jump, insertion, or prompt runs inside fenced code blocks.
- Multiple wikilinks on one line still choose the nearest marker candidate around the cursor.
- The normal successful file-link completion path does not show a notice.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/block-id-prompt/main.js
```

Focused Node checks using a small VM harness with stubbed `obsidian` and `@codemirror/view` modules:

- Parser accepts `[[path/to/foobar]]^` as file-link completion and computes:
  - `plainReplacement: [[path/to/foobar]]`
  - `completionReplacement: [[path/to/foobar^]]`
  - cursor after the inserted `^`.
- Parser accepts `[[path/to/foobar|foobar]]^` and removes `|foobar`.
- Parser accepts a non-block heading/subpath link such as `[[path/to/foobar#Heading|alias]]^` and inserts the `^` at the
  current inside-link end.
- Parser rejects:
  - `[[path/to/foobar#^old-id|alias]]^`
  - `[[path/to/foobar^old-id|alias]]^`
  - `[[path/to/foobar#^^old-id|alias]]`
  - `[[path/to/foobar]]^^`
- CM6 helper dispatches a single transaction with:
  - one replacement change,
  - final selection after the inserted `^`,
  - `userEvent: "input.type"`,
  - `scrollIntoView: true`.
- Editor fallback performs the plain replacement plus a `^` insertion at the computed cursor.

Manual live-vault smoke test after implementation and plugin reload:

1. In a scratch Markdown note, type or create `[[path/to/foobar]]`, put the cursor after the final `]`, and press `^`.
2. Confirm the link becomes `[[path/to/foobar^]]`, the cursor is after `^`, and Obsidian's block completion menu opens
   without a second keypress.
3. Select a block and confirm Obsidian completes the block link correctly.
4. Repeat with `[[path/to/foobar|foobar]]`; confirm the alias is removed and completion opens.
5. Repeat with `[[path/to/foobar#^old-id|alias]]`; confirm the block-ID rename modal opens instead.
6. Repeat inside a fenced code block; confirm nothing is rewritten.

Before finishing implementation later:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If implementation changes files under `~/bob`, commit only task-related vault changes with the required SASE git commit
workflow and leave unrelated pre-existing vault changes untouched.

## Risks

- Obsidian's built-in block completion is not a public API exposed to this plugin. The plan deliberately triggers it by
  creating the same text/cursor state as a typed `^`; live Obsidian testing is required.
- A plain editor `replaceRange("^")` may not be treated exactly like user typing by Obsidian's suggester. The CM6
  `userEvent: "input.type"` path and one-frame fallback are the mitigation.
- Combining alias deletion and `^` insertion into a single CM6 transaction may differ slightly from the current undo
  behavior. This is probably desirable because the whole action is one command, but manual undo should be checked.
- The existing scanner is debounce-based. The new action must remain synchronous and small, and scan suppression should
  cover the plugin's own edits without interfering with Obsidian's suggester.
- Standard Markdown links are not covered; the existing trigger and the described behavior are wikilink-based.
