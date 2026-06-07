---
create_time: 2026-06-07 08:24:22
status: done
prompt: sdd/prompts/202606/obsidian_cmd_n_new_note_template.md
---
# Plan: Make Cmd+N New Notes Use `new_note.md`

## Goal

When Bryan presses `Cmd+N` in the Bob Obsidian vault, the new note should be created from `~/bob/_templates/new_note.md`
instead of starting as a bare empty note.

The created note should keep the existing template behavior:

- `parent` is populated from the active or last-open note when possible.
- `created` is populated from the target file creation time.
- The body starts with `# <% tp.file.title %>`.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read the required Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and automation context before changing cmd+n new note behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must inspect status first, preserve unrelated pre-existing changes, and
  commit only task-related files with the SASE git commit workflow before terminating after any vault file changes.
- Inspected the live vault status. The vault is already dirty from unrelated changes, including
  `.obsidian/hotkeys.json`, `_templates/new_note.md`, plugin files, notes, and untracked notes. The Templater settings
  file is currently clean.
- Inspected:
  - `/home/bryan/bob/.obsidian/hotkeys.json`
  - `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
  - `/home/bryan/bob/.obsidian/plugins/templater-obsidian/main.js`
  - `/home/bryan/bob/_templates/new_note.md`
  - `/home/bryan/bob/.obsidian/daily-notes.json`
  - `/home/bryan/bob/.obsidian/templates.json`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- Reviewed prior related plans for Enter-link creation and `new_note.md` changes.
- No `bob-cli` subcommands or options are being added, so `memory/long/cli_rules.md` is not required.

## Current Findings

- `hotkeys.json` has no explicit `Mod+N` binding. The current `Cmd+N` behavior is therefore Obsidian's built-in new-note
  command, not a custom Bob or Templater command.
- Templater is installed and enabled. Its settings already have:
  - `trigger_on_file_creation: true`
  - `enable_folder_templates: true`
  - `templates_folder: "_templates"`
- The current folder-template rule is:

  ```json
  {
    "folder": "_templates",
    "template": "_templates/new_note.md"
  }
  ```

  That rule only applies to new empty Markdown files created inside the `_templates` folder. It does not apply to normal
  notes created in the vault root or other note folders.

- Templater's own settings UI text and local implementation say a global default folder template is configured on the
  root folder `/`. Its folder-template lookup walks from the new file's parent folder up to the root and returns the
  deepest matching rule.
- Templater also skips files under the configured template folder, so pointing the root folder template at
  `_templates/new_note.md` should not recursively template files created under `_templates`.
- `~/bob/_templates/new_note.md` already contains the desired Templater frontmatter. It is currently dirty from earlier
  user/config work; this task should not modify it unless implementation testing shows it is malformed.
- `bob-navigation-hotkeys` already uses Templater's `create_new_note_from_template(...)` API for missing-link creation,
  but that path is not needed for ordinary `Cmd+N` if Templater's file-creation trigger is configured correctly.

## Design

Use Templater's existing file-creation trigger as the primary fix.

Change only `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json` initially:

```json
"folder_templates": [
  {
    "folder": "/",
    "template": "_templates/new_note.md"
  }
]
```

Leave `hotkeys.json` alone for the primary implementation. This keeps Obsidian's built-in `Cmd+N` command in place: it
creates the empty note, then Templater fills that empty note from the root folder template rule.

Do not add a custom plugin command for the primary path. A custom command bound to `Mod+N` would have to compete with or
explicitly override Obsidian's built-in `file-explorer:new-file` default hotkey, which is more invasive than fixing the
existing Templater configuration.

## Fallback

If live testing shows that the root folder template does not trigger for `Cmd+N`, use a narrower hotkey-command
fallback:

1. Add `_templates/new_note.md` to Templater's `enabled_templates_hotkeys`.
2. Bind Templater's generated create command, `templater-obsidian:create-_templates/new_note.md`, to `Mod+N`.
3. Add or adjust a `file-explorer:new-file` hotkey entry only if necessary to stop the built-in default from also
   handling `Cmd+N`.

Only take this fallback after isolating and documenting why the root folder-template route fails.

## Implementation Scope

Expected vault file to edit after this plan is submitted:

- `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`

Expected files not to edit for the primary implementation:

- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/_templates/new_note.md`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
- Any `bob-cli` Rust source
- Any memory files

If the fallback is needed, the additional expected files would be:

- `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`

## Implementation Steps

1. Re-check `git -C /home/bryan/bob status --short` immediately before editing.
2. Apply a targeted JSON edit to replace the folder-template `folder` value from `_templates` to `/`, preserving the
   existing `template` value and surrounding settings.
3. Validate JSON syntax with `jq '.' /home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`.
4. Inspect the task diff and confirm it is limited to the intended Templater setting.
5. If Obsidian or Templater needs to reload settings, reload Obsidian or disable/enable Templater before manual testing.

## Verification

Static checks:

```bash
jq '.' /home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/templater-obsidian/data.json
git -C /home/bryan/bob diff -- .obsidian/plugins/templater-obsidian/data.json
```

Manual/live acceptance after reloading Obsidian or Templater:

1. Open an ordinary Markdown note so the template has a source note for `parent`.
2. Press `Cmd+N`.
3. Confirm the new note is populated from `_templates/new_note.md`.
4. Confirm the rendered note has `parent`, `created`, and a title heading.
5. Confirm the `parent` points at the source or last-open note rather than `_templates/new_note.md`.
6. Delete the scratch note after inspection if it was only created for testing.

Regression checks:

- Create or open today's daily note and confirm the Daily Notes workflow still uses `_templates/daily`, not
  `_templates/new_note.md`.
- Confirm creating or editing template files under `_templates` is not affected by the root folder-template rule.
- Confirm unrelated dirty vault files are unchanged.

## Risks

- The root folder-template rule is broader than just `Cmd+N`; it will apply to any new empty Markdown file in the vault
  unless a deeper folder-template rule overrides it. This is probably the desired default-new-note behavior, but daily
  notes and plugin-created notes need a smoke test.
- If Obsidian's Daily Notes plugin creates an empty note before inserting its own template, Templater might race and
  apply `new_note.md`. If this happens, revert the root folder-template approach and use the `Cmd+N`-specific fallback.
- Obsidian may not reload Templater's settings immediately after the JSON edit. A plugin reload or app reload may be
  needed before testing.
- The existing `_templates/new_note.md` file is already dirty. Do not normalize or reformat it as part of this task.

## Commit Plan

If implementation changes any files under `~/bob`, commit only the task-related file(s) with the SASE git commit
workflow, leaving unrelated dirty vault changes untouched.
