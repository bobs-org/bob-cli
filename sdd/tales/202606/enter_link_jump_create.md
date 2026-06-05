---
status: planned
create_time: 2026-06-05 14:42:06
prompt: sdd/prompts/202606/enter_link_jump_create.md
---

# Enter Link Jump/Create Plan

## Goal

Make Vim normal-mode Enter in the Bob Obsidian vault act as a robust line-target link opener/creator:

- A Vim count selects the target line relative to the current line. For example, `5<enter>` targets the line five lines
  below the cursor.
- If the target line has exactly one internal note link, Enter opens the linked note when it exists.
- If that one link points at a missing note, Enter creates the note using `~/bob/_templates/new_note.md`, then opens it.
- If the target line has multiple internal note links, Enter opens a filtered picker using the same interface as the
  child-note picker, then creates or opens the selected target.
- If the target line has no actionable internal note links, preserve the existing Enter behavior.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian note/template workflow context before planning link jump/create behavior for new_note.md"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect status first, preserve unrelated changes, and commit only
  task-related files with the SASE git commit workflow if edits are made under `~/bob`.
- Inspected prior approved plans for:
  - `sdd/tales/202606/obsidian_child_notes_popup.md`
  - `sdd/tales/202606/obsidian_child_popup_usability.md`
  - `sdd/tales/202606/obsidian_file_link_caret_jump.md`
- Inspected the current navigation plugin:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
- Inspected the current Enter owner:
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json`
- Inspected Vim and hotkey configuration:
  - `/home/bryan/bob/.obsidian.vimrc`
  - `/home/bryan/bob/.obsidian/hotkeys.json`
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`
- Inspected Templater configuration:
  - `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
  - `/home/bryan/bob/_templates/new_note.md`
- Current vault status is very dirty from unrelated note/generated changes. The task-related status currently shows:
  - `.obsidian/hotkeys.json` is modified by pre-existing user/config changes.
  - `_templates/new_note.md` is untracked.
  - `bob-navigation-hotkeys` and `task-status-cycler` plugin files are clean before implementation.
- No `bob-cli` Rust CLI subcommands or options are being added, so `memory/long/cli_rules.md` is not required for this
  task.

## Current Implementation Facts

- `bob-navigation-hotkeys` already owns note navigation behavior and has reusable helpers for:
  - Markdown active-file/context access.
  - Wikilink and Markdown-link parsing.
  - Obsidian link target normalization and resolution through `metadataCache.getFirstLinkpathDest(...)`.
  - Opening resolved links with `workspace.openLinkText(...)`.
  - A polished filtered `ChildNotePickerModal` with keyboard filtering, ArrowUp/ArrowDown, Ctrl+N/Ctrl+P, Enter, click,
    and namespaced `bob-cnp-*` CSS.
- `task-status-cycler` currently maps Vim normal-mode `<CR>` directly with `window.CodeMirrorAdapter.Vim.mapCommand`.
  Its action toggles open/done task statuses on the current line, otherwise falls through by moving to the next nonblank
  line.
- `obsidian-vimrc-support` `exmap ... obcommand ...` does not pass Vim counts to Obsidian command callbacks. Count
  support therefore needs to remain in a real CodeMirror Vim action that receives `actionArgs.repeat`.
- Templater exposes a command/API path for creating a new note from a template. Its command handler calls
  `templater.create_new_note_from_template(templateFile, folder, basename, open)`.
- The current `_templates/new_note.md` uses `tp.app.workspace.getLastOpenFiles()[0]` to choose the parent link. That is
  close to the desired behavior, but the implementation should prefer the Templater `tp.config.active_file` context when
  available so notes created from Enter reliably link back to the source note.

## Product Decisions

1. Keep parsing, resolving, opening, creating, and the picker in `bob-navigation-hotkeys`.
   - This plugin already owns parent/template/link navigation and the child-note picker UI.
   - The new picker should reuse the child picker interaction model and CSS rather than introducing another UI pattern.

2. Keep the existing `<CR>` Vim mapping in `task-status-cycler`, but delegate link handling to `bob-navigation-hotkeys`.
   - `task-status-cycler` already owns the active `<CR>` mapping.
   - Replacing it from another plugin would be load-order sensitive and could regress task toggling.
   - Passing `actionArgs.repeat` from `task-status-cycler` to a navigation-plugin method gives count support without
     changing `.obsidian.vimrc` or relying on `obcommand`.

3. Preserve current task toggling precedence for the uncounted/default Enter case.
   - If Enter is pressed on a current-line task status that `task-status-cycler` currently toggles, keep that behavior.
   - For explicit counts greater than one, treat the count as a line-target navigation request.
   - If no link action applies, fall through to line movement using the same count.

4. Define line targeting as "current line plus Vim repeat, clamped to the editor".
   - Default repeat is `1`, matching the existing "move to next line" fallback.
   - `5<enter>` targets `cursor.line + 5`, clamped to the last editor line.
   - The fallback movement should also honor the same repeat.

5. Treat actionable links as internal note targets only.
   - Support ordinary Obsidian wikilinks: `[[note]]`, `[[folder/note]]`, aliases, headings, block ids, and optional
     transclusion marker `![[note]]`.
   - Support Markdown links whose destination resolves to a vault note, including `.md` paths and angle-bracketed
     destinations.
   - Ignore bare URLs, external URI links, malformed links, and non-note destinations for this Enter workflow.
   - For links with heading/block subpaths, resolve/open the full link when the base note exists; if creating, create
     the base note and open it.

6. Resolve before creating.
   - Use Obsidian's `metadataCache.getFirstLinkpathDest(...)` with the active file as the source path.
   - This preserves existing behavior for bare note names, folder-relative targets, aliases, and ambiguous vault state.
   - Only create when the base note target cannot be resolved and the target is a safe vault-relative Markdown note
     path.

7. Use `_templates/new_note.md` for creation.
   - Prefer Templater's `templater.create_new_note_from_template(templateFile, folder, basename, true)` when the
     Templater plugin and template file are available.
   - Ensure the destination folder exists before calling Templater.
   - If Templater is unavailable, show a clear notice rather than silently creating a bare note.
   - Update `_templates/new_note.md` so `parent` prefers `tp.config.active_file.path` and falls back to
     `tp.app.workspace.getLastOpenFiles()[0]`.

8. Generalize the child-note picker carefully.
   - Introduce a small reusable filtered picker abstraction or adapt the existing modal so it can render both child-note
     files and link candidates.
   - Keep the existing child-note popup behavior, styling, keyboard controls, filtering, and open-on-click semantics.
   - For link candidates, show basename/title, target/path, and an "Open" vs "Create" cue while preserving the same
     overall `bob-cnp-*` interface.

9. Avoid unrelated configuration churn.
   - Do not edit `.obsidian/hotkeys.json`; it already has unrelated user changes.
   - Do not edit `.obsidian.vimrc`; count support cannot flow through that path anyway.
   - Do not edit `obsidian-vimrc-support`, `mrj-jump-to-link`, Templater plugin files, or `bob-cli` Rust source.

## Implementation Scope

Expected vault files to edit after this plan is submitted:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css` only if the reused picker needs minor generic
  row/status styling
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- `/home/bryan/bob/_templates/new_note.md`

Likely `bob-navigation-hotkeys/main.js` changes:

- Add helpers to read target-line text from a CodeMirror editor:
  - parse/normalize Vim repeat;
  - clamp target line;
  - get target line text.
- Add line-level link extraction:
  - return ordered candidates with raw link text, display label, target, base target, source path, resolved file if any,
    and action kind (`open` or `create`);
  - reuse or extend existing wikilink/Markdown parser helpers instead of adding regex-only parsing;
  - deduplicate exact duplicate targets on the same line only when they would perform the same action.
- Add `handleVimEnterLinkAction(cm, actionArgs)`:
  - require an active Markdown file/editor;
  - compute target line from repeat;
  - collect actionable link candidates from that line;
  - return `false` when none are found;
  - immediately open/create when exactly one candidate exists;
  - open a filtered picker when multiple candidates exist;
  - return `true` once the workflow has claimed Enter, even if the eventual async open/create shows a notice.
- Add `openOrCreateLinkCandidate(candidate)`:
  - capture active file position;
  - if `candidate.resolvedFile` exists, open via `openResolvedLink(...)`;
  - otherwise create the base note through Templater, then open the created file.
- Add safe creation helpers:
  - strip heading/block subpaths for creation;
  - reject external URLs, absolute paths, empty targets, and `..` traversal;
  - normalize `.md` extension handling;
  - create missing folders under the vault before Templater creation.
- Generalize the current `ChildNotePickerModal` or add a sibling `LinkCandidatePickerModal` that reuses the same
  filtering, keyboard navigation, and CSS classes.
- Export focused helpers under `module.exports.helpers` for Node VM tests.

Likely `task-status-cycler/main.js` changes:

- Pass `actionArgs` into `handleVimEnterToggle`.
- Add a local `getVimRepeat(actionArgs)` helper matching the existing Bob plugin style.
- Preserve current task-toggle behavior for default repeat on current-line open/done task statuses.
- If task toggle does not apply, call the loaded `bob-navigation-hotkeys` plugin method:
  `this.app.plugins.plugins["bob-navigation-hotkeys"]?.handleVimEnterLinkAction(cm, actionArgs)`.
- If the navigation plugin returns `true`, stop.
- Otherwise call `vimEnterFallthrough(cm, repeat)` and update that fallback to move `repeat` lines down.

Likely `_templates/new_note.md` change:

- Keep the same frontmatter shape and title body.
- Replace the current parent expression with a Templater expression that prefers `tp.config.active_file?.path`, then
  falls back to `tp.app.workspace.getLastOpenFiles()[0]`, and emits `[[path]]` only when a file can be resolved.

## Acceptance Criteria

- Pressing Enter with no actionable link on the next line still moves to the next nonblank line.
- Pressing `5<enter>` with no actionable link five lines down moves to that line.
- Pressing Enter when the target line has one existing wikilink opens that note.
- Pressing `5<enter>` when the target line five lines down has one existing wikilink opens that note.
- A line with one missing safe wikilink creates the target note from `_templates/new_note.md`, opens it, and gives it a
  `parent` link to the source note.
- A line with multiple actionable links opens the filtered picker; filtering and keyboard navigation work like the child
  note picker, and Enter/click opens or creates the selected target.
- Existing child-note picker behavior is unchanged.
- Existing task-status Enter toggling still works for the default current-line open/done task case.
- Existing link commands (`open-parent-note`, `open-template-note`, `open-next-link`, `open-prev-link`) still work.
- External links and unsafe targets are ignored or reported without creating files.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/styles.css \
  .obsidian/plugins/task-status-cycler/main.js \
  _templates/new_note.md
```

Focused Node VM checks with stubbed `obsidian` and `@codemirror/view` modules:

- Repeat parsing defaults to `1`, floors positive numbers, and clamps invalid repeats to `1`.
- Target line computation clamps at the editor's last line.
- Link extraction returns ordered candidates for:
  - `[[note]]`
  - `[[folder/note|Alias]]`
  - `![[note]]`
  - `[[note#Heading]]`
  - `[Label](folder/note.md)`
  - `[Label](<folder/note with spaces.md>)`
- Link extraction ignores external URLs and malformed links.
- A single resolved candidate calls the open path.
- A single unresolved safe candidate calls the Templater create path.
- Multiple candidates instantiate the filtered picker rather than opening the first candidate.
- The generalized picker preserves filtering, ArrowUp/ArrowDown, Ctrl+N/Ctrl+P, Enter, click, empty results, and
  selection clamping.
- `task-status-cycler` still toggles current-line open/done tasks for default Enter and delegates non-task/count Enter
  to the navigation plugin.

Manual live-vault acceptance check after implementation and plugin reload:

1. Create or use a scratch note with lines containing no link, one existing link, one missing link, and multiple links.
2. Press Enter above a no-link target and confirm ordinary movement.
3. Press Enter above a one-link target and confirm the existing note opens.
4. Press `5<enter>` above a target five lines down and confirm the count selects that line.
5. Press Enter above a missing-link target and confirm a new note is created through `_templates/new_note.md`, opened,
   and given the source note as `parent`.
6. Press Enter above a multiple-link target and confirm the filtered picker appears; filter, use Ctrl+N/Ctrl+P, press
   Enter, and confirm the chosen target opens or is created.
7. Press Enter on an existing open/done task line and confirm the old task toggle behavior remains.
8. Open the child-note picker and confirm it still looks and behaves the same.

Before finishing implementation:

```bash
git -C /home/bryan/bob status --short -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/styles.css \
  .obsidian/plugins/task-status-cycler/main.js \
  _templates/new_note.md \
  .obsidian/hotkeys.json \
  .obsidian.vimrc
git status --short
```

If vault files are changed, commit only the task-related vault files with `/sase_git_commit`, leaving the existing dirty
vault notes/generated files and pre-existing `.obsidian/hotkeys.json` changes untouched.

## Risks

- Plugin-to-plugin delegation depends on `bob-navigation-hotkeys` being loaded. The fallback should preserve current
  Enter movement if the navigation plugin is unavailable.
- Templater's internal API is not a formal Obsidian API. Guarding for missing methods and failing with a notice makes
  breakage obvious instead of creating malformed notes.
- Templater context for `tp.config.active_file` is the most reliable way to set `parent`, but live testing is still
  needed because template execution timing can vary by Templater version.
- Creating missing folders is convenient but must stay vault-relative and reject traversal to avoid accidental writes
  outside the vault.
