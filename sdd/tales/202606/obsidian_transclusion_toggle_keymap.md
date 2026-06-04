---
create_time: 2026-06-03 22:02:17
status: wip
prompt: sdd/prompts/202606/obsidian_transclusion_toggle_keymap.md
---
# Obsidian Line Transclusion Toggle Keymap Plan

## Goal

Add a Vim normal-mode `!` mapping in the Bob Obsidian vault that toggles link transclusion markers on the current editor
line.

Expected behavior:

- On a line with ordinary links, pressing `!` adds the Obsidian transclusion marker, turning `[[Note]]` into `![[Note]]`
  and `[label](target.md)` into `![label](target.md)`.
- On a line where all recognized links are already transcluded, pressing `!` removes the leading marker from each link.
- On a mixed line, pressing `!` normalizes the line to transcluded links by adding missing markers and leaving existing
  transclusions in place. Pressing `!` again then removes all transclusion markers.
- If the current line has no recognized links, show a small notice and leave the editor unchanged.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and vim-mode context before planning a transclusion toggle keymap"`.
- Read `/home/bryan/bob/AGENTS.md`; the vault is actively synced, so implementation must inspect status, preserve
  unrelated dirty note changes, and commit task-related vault changes with the required SASE commit workflow if edits
  are made under `~/bob`.
- Inspected the live navigation plugin: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- Inspected the live hotkey file: `/home/bryan/bob/.obsidian/hotkeys.json`.
- Inspected the live ledger plugin: `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- The vault currently has unrelated dirty notes and untracked note/reference paths. The expected target plugin files are
  tracked and currently clean.
- `bob-navigation-hotkeys` already registers Vim normal-mode mappings through `window.CodeMirrorAdapter.Vim`:
  - `[[` opens the previous labeled body link.
  - `]]` opens the next labeled body link.
- `bob-ledger-tools` has the established pattern for Vim actions that mutate the current editor line using the `cm`
  object passed by codemirror-vim.

## Product Decisions

1. Implement this in `bob-navigation-hotkeys`.
   - The behavior is link-oriented and belongs beside the existing link parsing and Vim navigation mappings.
   - No `bob-cli` Rust change is expected.

2. Use a line-level toggle, not per-link inversion.
   - If every recognized link on the current line is transcluded, remove one leading `!` from each.
   - Otherwise, add `!` to every recognized non-transcluded link and leave already-transcluded links as-is.
   - This gives predictable repeated-key behavior: first press embeds all links, second press returns all links to
     ordinary links.

3. Recognize the link forms Bryan is likely to edit in Obsidian notes.
   - Wikilinks: `[[target]]`, `[[target|alias]]`, `[[target#heading]]`, `![[target]]`.
   - Inline Markdown links: `[label](target)`, `[label](<target with spaces.md>)`, `![label](target)`.
   - Do not attempt to toggle bare URLs, reference-style Markdown links, malformed links, or unrelated bracket text.

4. Keep parsing narrow but safe.
   - Reuse the existing bracket/paren scanner helpers where practical.
   - Avoid touching wikilink-looking text that is part of Markdown-link bracket syntax.
   - Make one complete pass over the line, collect link-marker positions, then apply replacements from right to left so
     indexes remain stable.

5. Preserve the editor experience.
   - Keep the mapping normal-mode only via `vim.mapCommand("!", ..., { context: "normal" })`.
   - Preserve the current cursor line.
   - Adjust cursor column by inserted/removed markers before the cursor so the cursor remains on the same logical text
     after the rewrite.
   - Return `true` only when a line was changed.

## Implementation Scope

Expected vault file to edit:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`

No expected edits to:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`
- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`
- Any `bob-cli` Rust, script, README, or test files

Likely JavaScript changes:

- Add small editor helpers similar to `bob-ledger-tools`:
  - `getEditorCursor(cm)`
  - `getEditorLine(cm, line)`
  - `replaceEditorLine(cm, line, oldLineText, newLineText)`
  - `setEditorCursorSafely(cm, line, ch)`
- Add pure line helpers:
  - `findTransclusionToggleTargets(line)`
  - `toggleLineTransclusions(line)`
  - `adjustCursorChForTransclusionChanges(cursorCh, changes)`
- Add an action method on the plugin:
  - `toggleCurrentLineTransclusions(cm)`
  - validate editor/cursor/line access;
  - call the pure toggle helper;
  - show `Notice("No links found on current line")` for no-op lines;
  - replace the line and restore the adjusted cursor.
- Extend `registerVimMappings()`:
  - `vim.defineAction("bobNavigationToggleLineTransclusions", (cm) => this.toggleCurrentLineTransclusions(cm))`
  - `vim.mapCommand("!", "action", "bobNavigationToggleLineTransclusions", {}, { context: "normal" })`
- Export the pure helpers through `module.exports.helpers` for focused Node verification.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
jq '.' /home/bryan/bob/.obsidian/hotkeys.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Focused Node helper assertions with stubbed `obsidian` and `@codemirror/view` modules:

- `[[Note]]` becomes `![[Note]]`.
- `![[Note]]` becomes `[[Note]]`.
- Two ordinary wikilinks on one line both get `!`.
- Two transcluded wikilinks on one line both lose `!`.
- A mixed line such as `![[A]] [[B]]` becomes `![[A]] ![[B]]`.
- Markdown links toggle between `[label](target.md)` and `![label](target.md)`.
- Wikilink aliases, headings, block ids, and Markdown destinations with nested parentheses survive unchanged.
- Malformed bracket text and bare URLs are ignored.
- Cursor adjustment accounts for insertions/removals before the cursor and clamps to the rewritten line length.

Manual live-vault acceptance:

- Reload Obsidian or the `bob-navigation-hotkeys` plugin.
- Open a Markdown note in Vim mode.
- On a line with one or more ordinary wikilinks, press `!` and confirm each link becomes transcluded.
- Press `!` again on the same line and confirm the links return to ordinary links.
- Repeat with a line containing multiple links and a mixed transcluded/non-transcluded line.
- Confirm existing mappings still work: `[[`, `]]`, `Ctrl+6`, `Ctrl+-`, `Ctrl+.`, and `Ctrl+\`.

Before finishing:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If the vault plugin is edited, commit only `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` with the
required SASE commit workflow, leaving unrelated dirty vault notes untouched.

## Risks

- `!` is a built-in Vim normal-mode operator in full Vim. This plan intentionally overrides it in Obsidian Vim mode
  because the requested workflow is specific to link transclusion.
- Automated helper checks can validate parsing and editor mutation, but the actual single-key Vim binding needs a live
  Obsidian smoke test after plugin reload.
- Inline-code awareness is intentionally out of scope for the first pass. If that becomes annoying in practice, the pure
  scanner can be extended to skip code spans without changing the Vim mapping contract.
