---
create_time: 2026-06-06 08:27:18
status: wip
prompt: sdd/prompts/202606/fix_new_note_parent_nan.md
---
# Plan: Fix `new_note.md` Parent `NaN`

## Goal

Fix the Obsidian Enter link-create path so a missing-note jump creates the new note from `_templates/new_note.md` with
`parent` linking back to the source note, not `NaN`.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory with:
  `sase memory read long/obsidian.md --reason "Need Obsidian note workflow context before diagnosing Enter keymap new-note template behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault work must inspect status first, preserve unrelated dirty files, and commit
  only current-task vault changes before finishing.
- Inspected current vault status. `_templates/new_note.md` is already modified before this task, along with unrelated
  notes/config; preserve those unrelated changes and avoid broad vault churn.
- Inspected:
  - `/home/bryan/bob/_templates/new_note.md`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
  - `/home/bryan/bob/.obsidian/plugins/templater-obsidian/main.js`
  - `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
  - Prior SDD notes for Enter link creation and all-link-type support.

No `bob-cli` Rust CLI subcommands or options are being added, so `memory/long/cli_rules.md` is not required.

## Root Cause

`_templates/new_note.md` currently uses this parent expression:

```md
parent: "<%+ (() => { ... return `[[${path}]]`; })() %>"
```

Templater has a dynamic-template syntax that also uses `<%+ ... %>` in rendered Markdown, but during
`create_new_note_from_template(...)` this template is parsed as a normal expression whose JavaScript body starts with a
literal unary plus:

```js
+ (() => { ... return "[[source.md]]"; })()
```

When the IIFE returns the desired wikilink string, unary `+` converts that string to a number. `Number("[[source.md]]")`
is `NaN`, so the generated YAML gets `parent: "NaN"` / `parent: NaN`.

The Enter plugin's Templater API call itself is not the immediate bug: the installed Templater signature is
`create_new_note_from_template(templateFile, folder, filename, open = true)`, and `bob-navigation-hotkeys` passes those
arguments in that order. Templater builds `tp.config.active_file` from the currently active Obsidian file before opening
the newly created file, so the template can still use `tp.config.active_file?.path` once the expression syntax stops
coercing its return value.

## Implementation Plan

1. Update `/home/bryan/bob/_templates/new_note.md` only.
   - Change the `parent` command from `<%+ ... %>` to normal immediate interpolation `<% ... %>`.
   - Preserve the user's existing inline YAML form and `created` field.
   - Keep the existing source selection logic: prefer `tp.config.active_file?.path`, fall back to
     `tp.app.workspace.getLastOpenFiles()[0]`, and emit a wikilink only if `tp.file.find_tfile(path)` resolves.

2. Avoid plugin changes unless implementation/testing disproves the diagnosis.
   - `bob-navigation-hotkeys` already passes the expected Templater arguments.
   - A plugin-level parent post-write would be a broader change and should not be necessary for this specific `NaN`
     failure.

3. Preserve unrelated vault changes.
   - Do not touch `.obsidian/community-plugins.json`, dirty notes, generated notes, hotkeys, Templater settings, or
     `bob-cli` Rust source.
   - Because `_templates/new_note.md` is already dirty, edit only the one command marker and keep the pre-existing
     `created` line intact.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
git -C /home/bryan/bob diff --check -- _templates/new_note.md
```

Focused reasoning/repro check:

```bash
node -e 'console.log(+(() => "[[bob.md]]")())'
node -e 'console.log((() => "[[bob.md]]")())'
```

The first command demonstrates the current `NaN` coercion; the second demonstrates the intended output once the unary
plus is removed.

Manual acceptance after plugin/template reload:

1. In an existing source note, put the cursor on or target a line containing a missing internal link.
2. Press Enter through the existing Vim keymap.
3. Confirm the new note is created from `_templates/new_note.md`.
4. Confirm its frontmatter has `parent: "[[<source-note-path>]]"` and not `NaN`.
5. Confirm the `created` field still renders.

## Finish Criteria

- The plan is submitted with `sase plan sase_plan_fix_new_note_parent_nan.md` before vault edits.
- The only expected implementation diff is the `<%+` to `<%` change in `/home/bryan/bob/_templates/new_note.md`.
- After editing under `~/bob`, commit only that task-related template change using the required SASE git commit
  workflow.
