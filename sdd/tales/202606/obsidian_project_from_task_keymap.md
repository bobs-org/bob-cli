---
create_time: 2026-06-12 09:54:15
status: wip
prompt: sdd/prompts/202606/obsidian_project_from_task_keymap.md
---
# Obsidian `<Ctrl+Alt+Shift+N>` — Create Project Note From Selected Task

## Goal

Add a `<Ctrl+Alt+Shift+N>` keymap to Bryan's `~/bob` Obsidian vault that creates a new project note exactly like the
existing `<Ctrl+Shift+N>` keymap (template `_templates/new_project.md`), but seeds the project's `^prj`
completion-criteria task from the **task under the cursor** in the active note. After the project note is created:

1. The selected task's text replaces the `(REPLACE WITH PROJECT COMPLETION CRITERIA)` placeholder on the `^prj` line.
2. The original task line is deleted from the source note.
3. The new project note is open in the current tab (untitled, title in rename mode — same UX as `<Ctrl+Shift+N>`).
4. A useful toast summarizes what happened.

## Context Reviewed

- Workspace short memory: `memory/short/sase.md`. `memory/long/cli_rules.md` is **not** required: no bob-cli
  subcommands/options change (all work lives in the vault). If implementation deviates into the CLI surface, the agent
  MUST first run `/sase_memory_read` on `memory/long/cli_rules.md`.
- Vault instructions: `/home/bryan/bob/AGENTS.md` — vault is live under Obsidian Sync and currently dirty
  (`_templates/daily.md`, `_templates/new_project.md`, `bob_projects.md`, `dash.md`, `gtd_daily.md`, `mac_inbox.md`,
  `sase.md`, today's daily, two ref-chat notes). Inspect `git status` before editing; stage/commit only task files via
  `/sase_git_commit`.
- Live plugin: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (~4312 lines; plain JS maintained in
  place; pure helpers exported via `module.exports.helpers` for node smoke tests).
- Live hotkeys: `/home/bryan/bob/.obsidian/hotkeys.json`; live vimrc: `/home/bryan/bob/obsidian_vimrc.md`.
- Prior plans/tales: `sase_plan_obsidian_projects.md` (Phase 2 built `create-project-note`; Design Contract: `^anchors`
  are referenced from daily Pomodoro logs — never break them), `sase_plan_obsidian_alt_file_property_keymap.md`
  (keymap-addition conventions).
- Templater internals (installed `templater-obsidian/main.js`):
  `create_new_note_from_template(template, folder, filename, open_new_note=true)` opens the created note via
  `workspace.getLeaf(false)` — the **current tab** — in source mode, jumps to the template cursor location, and sets
  ephemeral `rename: "all"` state. Requirement (3) is therefore inherited for free by reusing the existing creation
  path.
- Backlink-inspection precedent: `block-id-prompt/main.js` uses `metadataCache.getBacklinksForFile` behind a
  function-existence guard and handles both `Map` and plain-object `data`.

## Current Findings

### How `<Ctrl+Shift+N>` works today

1. `hotkeys.json` binds `{"modifiers": ["Ctrl","Shift"], "key": "N"}` → `bob-navigation-hotkeys:create-project-note`.
2. `createProjectNote()` (main.js:3391): requires an active markdown file whose `type` resolves to `[[area]]` or
   `[[project]]` (`isAreaOrProjectNote`); aborts with a toast otherwise. Creates an untitled note in the vault root from
   `_templates/new_project.md` via Templater, then forces frontmatter via `processFrontMatter` (`parent` = wikilink to
   the creating note, `type: "[[project]]"`, `status: wip`) and shows `Created note: <path>`.
3. The template's first body line is the project completion-criteria task:
   `- [ ] #task (REPLACE WITH PROJECT COMPLETION CRITERIA) [p::2] ^prj`. **Note**: this placeholder text exists only in
   the template's _uncommitted_ working-tree version (Bryan's own pending edit) — the committed version still has
   `<short_project_completion_criteria_goes_here>` and a title heading. The feature builds on the working-tree contract
   Bryan referenced.

### Task conventions in this vault

- Tasks plugin global filter `#task`; custom statuses ` `=TODO, `/`=IN_PROGRESS, `B`=Blocked, `x`=done, `-`=Cancelled.
- Tasks carry inline Dataview fields (`[p::N]` priority, etc.) and often trailing block IDs (`^abc123`) that daily
  Pomodoro logs link to (`[[note#^abc123]]`) — deleting a referenced anchor breaks those links.

## Design

### UX flow

With the cursor on an open task line inside an area/project note, `<Ctrl+Alt+Shift+N>`:

1. Creates an untitled project note in the vault root from `_templates/new_project.md` (current tab, rename mode —
   identical to `<Ctrl+Shift+N>`, including the `parent`/`type`/`status` frontmatter enforcement).
2. Replaces `(REPLACE WITH PROJECT COMPLETION CRITERIA)` in the new note with the selected task's description; if the
   task carried `[p::N]`, that priority replaces the template's `[p::2]`.
3. Deletes the original task line from the source note.
4. Shows one toast, e.g. `Created project from task "Buy a new mattress…" (task removed from soon_dev)`.

### Validations (each failure → specific toast, nothing created/deleted)

- Active markdown view with an editor exists (command uses `editorCallback`, so Obsidian also greys it out otherwise).
- Active note is an area/project note (reuse `isAreaOrProjectNote`, same as `create-project-note`).
- Cursor line is a checkbox list item carrying the `#task` global filter, with an **open** status (` `, `/`, or `B`);
  done/cancelled tasks are rejected.
- The cleaned task description is non-empty.
- The next non-blank line is **not** a more-indented list item: a task with child bullets is rejected (toast tells Bryan
  to handle the children first) instead of silently orphaning or discarding them.
- If the task line ends in a block ID `^id`: inspect the source note's backlinks (`metadataCache.getBacklinksForFile`,
  guarded as in block-id-prompt) for any link with subpath `#^id`. If referenced (e.g. from a daily Pomodoro log), abort
  with an explanatory toast — per the projects Design Contract, anchors must never be broken. Unreferenced block IDs are
  simply dropped (the new line's anchor is `^prj`). If the backlinks API is unavailable, skip the check and proceed.

### Building the `^prj` line

From the captured task line, extract the **description**: strip leading indent + bullet + checkbox marker, remove the
standalone `#task` tag token, strip a trailing block ID, and extract-and-remove a `[p::N]` field (recording `N`);
collapse runs of whitespace. Everything else (other inline fields, dates, links, formatting) is preserved verbatim. In
the created note, string-replace the placeholder (module constant
`PROJECT_COMPLETION_PLACEHOLDER = "(REPLACE WITH PROJECT COMPLETION CRITERIA)"`) with the description, and `[p::2]` →
`[p::N]` when a source priority was found. The `^prj` task always starts as `- [ ]` (TODO), even if the source was `/`
or `B`.

### Order of operations & failure semantics

1. Validate + capture (cursor line number and exact text) from the live editor.
2. `await view.save()` — flush the source editor buffer to disk before navigating away (guards the read-modify-write
   below against unsaved edits).
3. Create the project note (refactor: extract the template-creation + frontmatter core of `createProjectNote()` into a
   shared private helper so both commands use one code path; `createProjectNote()` behavior is unchanged).
4. `vault.process(createdFile, …)` to substitute the placeholder/priority. **If the placeholder is absent** (template
   drifted), show a warning toast and stop — the user keeps a normal new project note AND the original task.
5. `vault.process(sourceFile, …)` to delete the original task line: remove the captured line number if its text still
   matches exactly, else a unique exact-text match; if not found, warn via toast (no deletion) — the project note is
   still valid. The task is only ever deleted after its text has landed in the new note.
6. Success toast (pure helper, e.g. `getProjectFromTaskNoticeText`): truncated description + source note basename.

## Implementation Steps

1. **`/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`** (no manifest change):
   - Register `create-project-note-from-task` ("Create project note from task") in `onload()` immediately after
     `create-project-note`, using `editorCallback: (editor, view) => this.createProjectNoteFromTask(editor, view)`.
   - Add `createProjectNoteFromTask()` after `createProjectNote()`, implementing the flow above; extract the shared
     creation core out of `createProjectNote()`.
   - Add pure helpers near the other line-parsing helpers and export them via `module.exports.helpers`:
     `parseProjectSourceTaskLine(lineText)` → `{description, priority, blockId, status}` | `null`;
     `buildProjectContentFromTask(content, parsedTask)` → updated content | `null` (placeholder missing);
     `removeTaskLineFromContent(content, lineNumber, lineText)` → `{content, removed}`;
     `getProjectFromTaskNoticeText(description, sourceBasename)`.
2. **`/home/bryan/bob/.obsidian/hotkeys.json`**: add
   `"bob-navigation-hotkeys:create-project-note-from-task": [{"modifiers": ["Alt","Ctrl","Shift"], "key": "N"}]` next to
   the other `bob-navigation-hotkeys` entries. No conflict: the only other `N` binding is `Ctrl+Shift+N`, and no
   Obsidian default uses `Ctrl+Alt+Shift+N`.
3. **No changes** to `obsidian_vimrc.md` (precedent: `create-project-note` needs none — Obsidian hotkeys with
   Ctrl+Alt+Shift chords fire in all vim modes), `_templates/new_project.md`, plugin `manifest.json`, or bob-cli code.

## Validation

1. Static: `node --check …/bob-navigation-hotkeys/main.js`; `jq '.' …/hotkeys.json`.
2. Node smoke test (established mocked-`obsidian` pattern): plugin loads, `create-project-note-from-task` is registered,
   and `create-project-note` is still registered/unchanged.
3. Helper unit checks via `module.exports.helpers`: plain task; task with `[p::0]`; task with block ID; task with other
   inline fields preserved; done/cancelled task rejected; non-task and plain-bullet lines rejected; placeholder
   substitution incl. priority override; placeholder-missing returns `null`; exact-line removal incl. moved-line and
   missing-line cases.
4. Manual acceptance (Bryan, after reload — agents cannot run the Obsidian GUI):
   - From a project/area note with the cursor on an open `#task` line: `<Ctrl+Alt+Shift+N>` opens an untitled project in
     the current tab whose `^prj` line contains the task text (and its `[p::N]` if it had one); the original task line
     is gone; the toast reads well.
   - On a task with a Pomodoro-referenced block ID: aborts with the anchor-safety toast.
   - On a non-task line / done task / non-area note: correct error toast, nothing created or deleted.
   - `<Ctrl+Shift+N>` still behaves exactly as before.

## Risks / Notes

- **Vault is live and dirty** (Obsidian Sync): re-check `git -C ~/bob status --short` immediately before editing;
  re-read files right before writing; never touch the unrelated dirty files.
- **Placeholder contract lives in an uncommitted template edit**: this plan does NOT stage `_templates/new_project.md`
  (pre-existing change owned by Bryan, per vault AGENTS.md). If the working-tree template is ever reverted, the feature
  degrades gracefully (warning toast; task preserved). Bryan may want to commit his template edit separately so the
  contract is durable.
- **Anchor-abort decision**: refusing to delete a backlink-referenced task is intentionally conservative (Design
  Contract). Bryan can remove the referencing link or convert manually in those rare cases.
- **Child-bullet abort decision**: moving a task's sub-bullets into the project is explicitly out of scope; aborting
  beats silent data loss.
- **Editor/disk race** on the source note is mitigated by `view.save()` + `vault.process` (atomic read-modify-write),
  and by only deleting an exact-matching line.

## Commit Workflow

Per `/home/bryan/bob/AGENTS.md`: stage **only** `.obsidian/plugins/bob-navigation-hotkeys/main.js` and
`.obsidian/hotkeys.json`, and commit via the `/sase_git_commit` skill before terminating.
