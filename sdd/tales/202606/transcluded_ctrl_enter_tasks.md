---
create_time: 2026-06-06 11:10:39
status: done
prompt: sdd/prompts/202606/transcluded_ctrl_enter_tasks.md
---
# Plan: Ctrl+Enter Completion for Transcluded Obsidian Tasks

## Goal

Extend the existing Obsidian Vim normal-mode `<Ctrl+Enter>` task completion keymap so it works on a transcluded task
block line such as:

```md
- ![[bob#^ctrl-enter-transclude]]
```

When the active line contains a transcluded wikilink to a block ID, and that block ID resolves to an open/done Markdown
task line, `<Ctrl+Enter>` should toggle that source task through the same open/done semantics as the current direct task
line path. For Bryan's `#task` lines, completing should add `[completion:: YYYY-MM-DD]` before the trailing block ID and
reopening should remove the completion field, matching the current local fallback behavior.

## Context Reviewed

- Read `memory/short/sase.md`.
- Read Obsidian long-term memory with:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and task/transclusion workflow context before planning ctrl-enter behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect status first, preserve unrelated dirty changes, and commit
  only current-task vault edits before finishing.
- Inspected the live plugin: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- Inspected the link-resolution patterns in: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
- Reviewed prior plans for task completion metadata, Ctrl+Enter migration, full link/block-link support, and the current
  Ctrl+] checkbox-marker work.

No `bob-cli` Rust CLI subcommands or options are involved, so `memory/long/cli_rules.md` is not required.

## Current State

- `<Ctrl+Enter>` is already registered in `task-status-cycler/main.js` as both `<C-CR>` and `<C-Enter>`, and both call
  `handleVimTaskToggleOpenDone()`.
- Direct task-line behavior is:
  - parse the active editor line with `TASK_LINE_RE`;
  - only toggle open/done statuses (`[ ]` and `[x]` / `[X]`);
  - use `toggleActiveCheckboxOpenDone()` -> `setActiveCheckboxStatus()`;
  - prefer Obsidian Tasks commands for active `#task` lines;
  - fall back to local metadata rewriting that adds/removes `[completion:: YYYY-MM-DD]` and keeps trailing `^block-id`
    final.
- A transcluded task line like `  - ![[bob#^ctrl-enter-transclude]]` is currently not a task line, so Ctrl+Enter no-ops.
- The concrete prompt task resolves to `/home/bryan/bob/bob.md`, line containing:
  `- [ ] #task Add <ctrl+enter> support for transcluded task bullets! [created::2026-06-06] ^ctrl-enter-transclude`.
- The vault is already dirty in unrelated files. Relevant pre-existing dirty state includes:
  - `.obsidian/plugins/task-status-cycler/main.js` from the in-progress Ctrl+] checkbox-marker change;
  - `.obsidian/hotkeys.json`;
  - `bob.md` and `2026/20260606_day.md`. These changes must be preserved and not reverted or staged as part of this task
    unless they are directly edited here.

## Product Decisions

1. **Keep the keymap unchanged.**
   - Do not add a new hotkey or alter `.obsidian/hotkeys.json`.
   - The existing `<C-CR>` / `<C-Enter>` normal-mode action remains the entry point.

2. **Direct task lines keep priority.**
   - If the active line itself is an open/done task, keep using the existing direct-task path.
   - Only attempt transcluded task resolution when the active line is not an open/done task.

3. **Support Obsidian wikilink embeds to block IDs.**
   - Recognize `![[note#^block-id]]`, `![[note.md#^block-id]]`, `![[folder/note#^block-id|alias]]`, and same-file
     `![[#^block-id]]`.
   - Do not broaden this first pass to headings, non-embedded links, Markdown image/link syntax, non-markdown embeds, or
     task query output.

4. **Use Obsidian link resolution semantics.**
   - Resolve the note path with the same approach used by `bob-navigation-hotkeys`: normalize the target, strip the
     `#^block-id` subpath for `metadataCache.getFirstLinkpathDest()`, and treat a pure `#^block-id` target as the
     active/source file.
   - Reject unresolved files and non-markdown files.

5. **Resolve the source line by block ID, then require it to be a task.**
   - Prefer `app.metadataCache.getFileCache(file).blocks` position data when available.
   - Defensively support both common block-cache shapes: object keyed by ID and arrays of block entries.
   - If the cache is missing or stale, fall back to scanning the target file text for a standalone `^block-id` token.
   - After locating a candidate line, require `TASK_LINE_RE` and open/done status before editing.

6. **Use existing local line-rewrite semantics for transcluded source edits.**
   - The Obsidian Tasks command path acts on the active editor, so it is not suitable for a task line in another file.
   - For transcluded targets, rewrite the source line text directly using the same local helpers already used when Tasks
     commands are unavailable:
     - `#task` completion adds or replaces `[completion:: YYYY-MM-DD]`;
     - reopening removes `[completion:: ...]`;
     - trailing block IDs stay final;
     - non-`#task` checklists only change the checkbox symbol.
   - This intentionally does not try to emulate advanced Tasks recurrence behavior for non-active-file source edits.

7. **Avoid ambiguous toggles.**
   - If the active line has exactly one supported transcluded block link, use it.
   - If it has multiple supported transcluded block links, use the one under the cursor when possible.
   - If multiple candidates remain ambiguous, no-op rather than toggling the wrong task.

## Implementation Scope

Expected vault file to edit:

- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No expected edits to:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian.vimrc`
- `bob-cli` Rust source, scripts, README, tests, or memory files
- vault notes, except optional scratch-note manual smoke testing after implementation

## Implementation Approach

1. Re-check vault status before editing.
   - Capture the pre-change diff for `.obsidian/plugins/task-status-cycler/main.js` so the existing Ctrl+] work is not
     accidentally reverted or committed as this task's work.

2. Add small pure helpers near the existing task-line helpers.
   - Parse embedded wikilinks on a line and return candidate spans:
     - `target`;
     - `pathPart`;
     - `blockId`;
     - `startIndex` / `endIndex`.
   - Split aliases at `|`, strip `.md` from the path part, decode safe URI text, and reject empty/degenerate block IDs.
   - Add an escaping helper for block-ID scan regexes.
   - Export useful pure helpers under `module.exports.helpers` for focused Node checks.

3. Add arbitrary-line task parsing and rewrite helpers.
   - Factor `getActiveTaskStatus(editor)` through a new helper like `getTaskStatusForLine(lineText, lineNumber)`.
   - Add `getNextOpenDoneSymbol(taskStatus)` or equivalent so direct and transcluded paths share the same open/done
     toggle decision.
   - Add a pure helper that returns the rewritten task line for a source target without mutating an editor.

4. Add source resolution methods on the plugin class.
   - `getActiveLineTranscludedTaskTarget(editor)`:
     - read active cursor, line text, and active file path;
     - collect supported embed candidates;
     - pick the unique or cursor-contained candidate.
   - `resolveTranscludedBlockTarget(candidate, sourcePath)`:
     - resolve the target file through `metadataCache.getFirstLinkpathDest()` or active file for same-file embeds;
     - locate the block line via file cache or file-text scan;
     - return `{ file, line, lineText, taskStatus }` only when the block line is open/done task.

5. Add source line mutation.
   - Prefer `app.vault.process(file, callback)` if available so the read/modify/write cycle is serialized by Obsidian.
   - Fall back to `vault.read(file)` + `vault.modify(file, updatedText)` only if `process` is unavailable.
   - Preserve line endings when replacing exactly one line.
   - If the target file is the active editor's file and the located source line is available in the editor buffer, use
     `editor.replaceRange()` for that line to avoid clobbering active unsaved editor state.
   - Re-validate the block line inside the write callback before changing it, so stale metadata does not rewrite the
     wrong line.

6. Wire the behavior into Ctrl+Enter.
   - Update `handleVimTaskToggleOpenDone()`:
     - first keep the existing active-line task toggle;
     - if no active direct task was toggled, attempt the transcluded block-task toggle;
     - otherwise no-op as it does today.
   - Keep command-palette `toggle-task-open-done` scoped to direct active task lines unless implementation shows a clear
     reason to make command-palette behavior also target transclusions. The requested behavior is specifically the Vim
     Ctrl+Enter keymap.

## Acceptance Criteria

- Pressing `<Ctrl+Enter>` on `  - ![[bob#^ctrl-enter-transclude]]` marks the task line in `bob.md` done.
- The resulting source line keeps `^ctrl-enter-transclude` as the final token and gains a local-date
  `[completion:: YYYY-MM-DD]` field before it.
- Pressing `<Ctrl+Enter>` on the same transcluded line again reopens the source task and removes the completion field,
  matching the existing open/done toggle semantics.
- `![[bob.md#^ctrl-enter-transclude]]`, aliased embeds, foldered note embeds, and same-file `![[#^id]]` work.
- A transcluded block that resolves to a non-task line no-ops.
- A transcluded task with status other than open/done, such as `[/]` or `[-]`, no-ops.
- A normal direct task line still toggles exactly as before.
- `<Enter>` link behavior, `<Ctrl+]>`, Backspace link behavior, `o`, `<C-d>`, and `<C-u>` mappings remain unchanged.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/task-status-cycler/main.js
```

Focused Node checks with stubbed `obsidian`, fake app/vault/metadata cache, and fake editors:

- Parser recognizes `![[bob#^ctrl-enter-transclude]]`, `![[bob.md#^id|Alias]]`, `![[folder/note#^id]]`, and `![[#^id]]`.
- Parser ignores plain `[[bob#^id]]`, `![[bob#Heading]]`, `![[image.png]]`, malformed embeds, and degenerate block IDs.
- Resolver uses `metadataCache.getFirstLinkpathDest()` for cross-file embeds and the active source file for same-file
  embeds.
- Block lookup succeeds through object-shaped cache, array-shaped cache, and file-text fallback scan.
- Open `#task` source line rewrites to `[x]` and inserts completion metadata before the trailing block ID.
- Completed `#task` source line rewrites to `[ ]` and removes completion metadata.
- Non-`#task` source task only changes the checkbox symbol.
- Non-task block target and non-open/done task status produce no write.
- Direct active task-line Ctrl+Enter still calls the existing direct path and does not attempt transclusion resolution.
- Ambiguous multiple transclusions no-op unless the cursor is inside exactly one candidate span.

Manual smoke test after plugin reload or disable/enable:

1. In `2026/20260606_day.md`, put the cursor on `  - ![[bob#^ctrl-enter-transclude]]`.
2. Press `<Ctrl+Enter>` and confirm the source task in `bob.md` becomes done with today's completion field.
3. Press `<Ctrl+Enter>` again on the transcluded line and confirm the source task reopens and the completion field is
   removed.
4. Repeat with a scratch same-file embed `![[#^scratch-id]]`.
5. Confirm a normal direct task line still toggles through the existing Ctrl+Enter behavior.

Before finishing implementation:

```bash
git -C /home/bryan/bob status --short -- \
  .obsidian/plugins/task-status-cycler/main.js \
  .obsidian/hotkeys.json \
  bob.md \
  2026/20260606_day.md
git -C /home/bryan/bob diff -- .obsidian/plugins/task-status-cycler/main.js
git status --short
```

If implementation changes files under `/home/bryan/bob`, commit only this task's vault changes with the required
`sase_git_commit` workflow, leaving pre-existing dirty vault note/hotkey/plugin changes unrelated to this task alone.

## Risks

- **Non-active-file Tasks behavior.** The Obsidian Tasks command path is active-editor based, so transcluded source
  edits use the local fallback instead. This preserves Bryan's completion-field convention but does not implement
  advanced Tasks recurrence behavior.
- **Stale metadata cache.** Block positions can lag. Re-validating the block ID and task status during the write, plus
  falling back to a file scan, reduces the chance of editing the wrong line.
- **Pre-existing dirty plugin file.** `task-status-cycler/main.js` already has unrelated Ctrl+] edits. Keep changes
  narrowly scoped, inspect diffs before/after, and commit only the hunks from this task if implementation proceeds.
- **Ambiguous multi-embed lines.** No-op on ambiguity is safer than guessing; cursor-contained selection still supports
  deliberate multi-embed usage.
