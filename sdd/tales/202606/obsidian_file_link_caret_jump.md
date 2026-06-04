---
create_time: 2026-06-04 13:02:18
status: done
prompt: sdd/prompts/202606/obsidian_file_link_caret_jump.md
---
# Obsidian File Link Caret Jump Plan

## Goal

Revise the `block-id-prompt` trailing-`^` workflow so pressing `^` immediately after the final `]` of a non-block
Obsidian wikilink prepares that link for Obsidian's normal block completion by moving the cursor inside the link, just
before the closing `]]`.

For aliased file links, delete the alias portion first. In concrete terms:

```markdown
[[path/to/foobar|foobar]]^
```

should become:

```markdown
[[path/to/foobar]]
```

with the cursor at:

```markdown
[[path/to/foobar|]]
```

where `|` represents the cursor position before the closing brackets, not a literal pipe character.

For non-aliased file links:

```markdown
[[path/to/foobar]]^
```

should become:

```markdown
[[path/to/foobar]]
```

with the cursor at the same inside-link position:

```markdown
[[path/to/foobar|]]
```

Block links remain owned by the existing block rename functionality. For example:

```markdown
[[path/to/foobar#^old-id|foobar]]^
```

must still delete the transient trailing marker and open the block ID prompt rather than doing the file-link cursor
jump.

## Interpretation

The typed trailing `^` remains a transient command marker, as it does in the current plugin. The intended "move 2 to the
left" is therefore:

1. Detect a single `^` typed immediately after `]]`.
2. Delete that trailing `^`.
3. If the wikilink has an alias suffix, delete the entire suffix starting at the first `|`.
4. Place the cursor two columns left of the post-rewrite end of the wikilink, which is immediately before the closing
   `]]`.

This preserves the user-facing effect of moving from "after the file link" to "inside the file link where the block
completion caret belongs." If the implementation literally moved two columns left while leaving the typed `^` in place,
the cursor would land between the two closing brackets or leave a stray command marker, neither of which matches the
existing workflow or the desired second-step block completion behavior.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault/plugin workflow context before planning block-id-prompt behavior changes"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect vault status first, preserve unrelated changes, and commit
  only task-related vault files with the SASE git commit workflow if edits are made under `~/bob`.
- Inspected the approved prior plan at `sdd/tales/202606/obsidian_alias_block_completion_cursor.md`.
- Inspected `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`.
- Confirmed the previous implementation is in commit `8fae4d9 feat: jump to alias separator for block completion`.
- Current vault status has unrelated pre-existing changes outside `.obsidian/plugins/block-id-prompt/main.js`; later
  implementation must not stage, revert, or overwrite them.

## Current Implementation Facts

- The plugin scans CodeMirror document changes via `EditorView.updateListener`, debounced by `SCAN_DEBOUNCE_MS`.
- `inspectActiveEditor()` requires an active Markdown view, a single cursor, an active Markdown file, editor mutation
  APIs, and no fenced code block at the cursor line.
- `findMarkerLinkNearCursor()` scans wikilinks on the current line and chooses the nearest parsed marker candidate.
- Block prompt parsing currently wins through:
  - `parseInlineMarkerLink()` for inline `#^^old-id` markers.
  - `parseTrailingMarkerLink()` for a single trailing `^` after wikilinks whose destination already ends in a block
    reference, including `#^old-id` and the currently supported bare `^old-id` form.
- The prior alias behavior is implemented by `parseTrailingAliasJumpMarker()` and `applyAliasJumpMarker()`.
- That prior behavior only recognizes aliased non-block links and preserves the alias, placing the cursor before the
  alias pipe. The new behavior should replace this with a broader non-block file-link jump that removes aliases.

## Product Decisions

1. Keep this in `block-id-prompt`.
   - The plugin already owns trailing-`^` command markers after wikilinks.
   - No hotkey, CLI, or separate Obsidian plugin change is needed.

2. Preserve block rename precedence.
   - Existing block-marker parsing must run before the new file-link jump parser.
   - If a destination parses as a block reference, the new file-link jump parser must return `null`.
   - Existing modal save/cancel behavior should remain unchanged.

3. Apply the new jump to non-block wikilinks with or without aliases.
   - `[[file]]^` should now be consumed by the plugin and move inside the link.
   - `[[file|alias]]^` should remove `|alias`, consume the trailing marker, and move inside the resulting link.
   - `[[file#Heading|alias]]^` is not a block link, so it should follow the same file-link jump behavior unless later
     manual testing shows Obsidian cannot open useful block completion from that position.

4. Delete the alias suffix exactly from the first `|`.
   - Reuse `splitWikiLinkBody()`, matching the plugin's existing first-pipe alias boundary.
   - Preserve the destination text exactly as written rather than normalizing or trimming it in the replacement.
   - Use trimming only as a guard to avoid acting on an empty destination.

5. Keep safety behavior from the current plugin.
   - Require a single trailing marker and keep the `^^` guard.
   - Do nothing inside fenced code blocks.
   - Suppress follow-up scans around the plugin's own edit.
   - Do not show a notice for the normal jump path.

## Implementation Scope

Expected file to edit after this plan is approved:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js`

No expected edits to:

- `/home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/community-plugins.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/*`
- Any `bob-cli` source, script, README, or test fixture

Likely JavaScript changes:

1. Replace `parseTrailingAliasJumpMarker()` with a parser such as `parseTrailingFileLinkJumpMarker(match, lineText)`.
   - Confirm `lineText[markerCh] === "^"` and `lineText[markerCh + 1] !== "^"`.
   - Split `match[1]` with `splitWikiLinkBody()`.
   - Require `normalizeText(destination)` to be non-empty.
   - Return `null` when `parseBlockReferenceDestination(destination, { allowPathBareBlock: true })` succeeds.
   - Build a replacement wikilink without the alias suffix: `[[${destination}]]`.
   - Return an action object:
     - `kind: "file-link-jump"`
     - `raw: raw + "^"`
     - `replacement: [[destination]]`
     - `startCh: match.index`
     - `endCh: markerCh + 1`
     - `cursorCh: match.index + 2 + destination.length`

2. Preserve marker parse order in `parseMarkerLink()`.
   - Keep `parseInlineMarkerLink(match)` first.
   - Keep `parseTrailingMarkerLink(match, lineText)` second.
   - Try the new file-link jump parser last.
   - This ordering ensures block rename behavior wins for block links.

3. Rename or replace the editor action method.
   - Change the `marker.kind` branch in `inspectActiveEditor()` from `"alias-jump"` to `"file-link-jump"`.
   - Replace or rename `applyAliasJumpMarker()` to something like `applyFileLinkJumpMarker()`.
   - Continue verifying that the current line slice still equals `marker.raw` before mutating.
   - Call `suppressEditorScans()`, replace the marker span with `marker.replacement`, set the cursor to
     `{ line, ch: marker.cursorCh }`, and clear `lastPromptKey`.

4. Keep the rest of the block ID prompt path untouched.
   - `sourceKey()`, modal open/cancel/save, source replacement, block-reference collection, and markdown-link rewrite
     behavior should not change.

## Acceptance Criteria

- `[[path/to/foobar]]^` is rewritten to `[[path/to/foobar]]` with the cursor before the closing `]]`.
- `[[path/to/foobar|foobar]]^` is rewritten to `[[path/to/foobar]]` with the cursor before the closing `]]`.
- The cursor column for both cases is `match.index + 2 + destination.length`, which is two columns left of the rewritten
  wikilink end.
- Pressing `^` again from that cursor position leaves Obsidian responsible for normal block completion.
- `[[path/to/foobar#^old-id|foobar]]^` still opens the existing block ID prompt.
- Existing inline marker behavior such as `[[path/to/foobar#^^old-id|foobar]]` still opens the block ID prompt.
- Existing supported bare block-target behavior such as `[[path/to/foobar^old-id|foobar]]^` still opens the block ID
  prompt.
- `[[path/to/foobar]]^^` is not consumed by the new file-link jump path.
- No jump or prompt runs inside fenced code blocks.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/block-id-prompt/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/block-id-prompt/main.js
```

Focused behavior checks with a small Node VM harness that stubs `obsidian` and `@codemirror/view`:

- Non-aliased file link:
  - input line `[[path/to/foobar]]^`
  - output line `[[path/to/foobar]]`
  - cursor column immediately before `]]`
- Aliased file link:
  - input line `[[path/to/foobar|foobar]]^`
  - output line `[[path/to/foobar]]`
  - cursor column immediately before `]]`
- Heading or subpath non-block link:
  - input line `[[path/to/foobar#Heading|foobar]]^`
  - output line `[[path/to/foobar#Heading]]`
  - cursor column immediately before `]]`
- Block prompt precedence:
  - `[[path/to/foobar#^old-id|foobar]]^` resolves to the modal path, not file-link jump.
  - `[[path/to/foobar^old-id|foobar]]^` resolves to the modal path, preserving current bare-block support.
  - `[[path/to/foobar#^^old-id|foobar]]` resolves to the modal path.
- Guard behavior:
  - `[[path/to/foobar]]^^` is ignored.
  - Matching text inside a fenced code block is ignored.
  - Multiple wikilinks on one line still choose the nearest candidate to the cursor.

Manual live-vault acceptance check after implementation and plugin reload:

1. In a Markdown note, type or create `[[path/to/foobar]]`, put the cursor after the final `]`, and press `^`.
2. Confirm the trailing marker disappears and the cursor lands before the closing `]]`.
3. Press `^` again and confirm Obsidian opens its normal block completion for `path/to/foobar`.
4. Repeat with `[[path/to/foobar|foobar]]` and confirm the alias suffix is removed before the cursor lands inside the
   link.
5. Repeat with an existing block link such as `[[path/to/foobar#^old-id|foobar]]`; confirm the block ID prompt still
   opens.

Before finishing implementation later:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If implementation changes any file under `~/bob`, commit only the task-related plugin file with the required
`/sase_git_commit` workflow and leave unrelated vault changes untouched.

## Risks

- Standard Markdown links are not covered by this plan; the existing trigger is wikilink-based and the request appears
  to refer to Obsidian file wikilinks.
- Obsidian owns the second `^` block completion popup. The plugin can only place the cursor where Obsidian expects it.
- The debounce-based trigger means the action must stay small and synchronous, matching the current design.
- Removing aliases is intentionally destructive for the link text. That is now requested, but manual testing should
  confirm it is acceptable for all common completion paths.
