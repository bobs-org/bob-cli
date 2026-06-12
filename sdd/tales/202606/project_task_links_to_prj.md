---
create_time: 2026-06-12 18:46:30
status: done
prompt: sdd/prompts/202606/project_task_links_to_prj.md
---
# Plan: Migrate Promoted Task Block Links to Project `^prj`

## Goal

When the Obsidian `create-project-note-from-task` command runs from `<Ctrl+Shift+Alt+N>` on a task with a trailing block
ID, keep the existing project-note promotion flow but rewrite backlinks to the new project task anchor instead of the
new note root.

Example:

- Source task: `~/bob/foo_bar.md` task ending in `^baz`
- Created project note: `~/bob/foo_bar_baz.md`
- Current migrated backlink target: `[[foo_bar_baz]]`
- Desired migrated backlink target: `[[foo_bar_baz^prj]]`

Aliases and embeds should keep their existing display/transclusion shape, e.g. `[[foo_bar#^baz|alias]]` becomes
`[[foo_bar_baz^prj|alias]]` and `![[foo_bar#^baz]]` becomes `![[foo_bar_baz^prj]]`.

## Context Reviewed

- Project short memory: `memory/short/sase.md`.
- Required Obsidian long memory via:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and block-link workflow context before changing project-note promotion behavior"`.
- Plan workflow from `/home/bryan/.codex/skills/sase_plan/SKILL.md`.
- Current repo prior design notes:
  - `sdd/tales/202606/obsidian_project_from_task_block_id.md`
  - `sdd/tales/202606/project_note_default_filenames.md`
- Live vault instructions: `/home/bryan/bob/AGENTS.md`.
- Live implementation: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.

No `bob-cli` subcommands or options are being added, so `memory/long/cli_rules.md` is not required.

## Current Findings

- The relevant runtime code lives in the live Obsidian vault, not the Rust CLI workspace.
- The vault is already dirty, including `.obsidian/plugins/bob-navigation-hotkeys/main.js`; the existing plugin diff is
  unrelated dash-task navigation work. Preserve it and stage only this task's hunk if committing.
- The project-from-task flow already:
  - parses the source task and its trailing block ID,
  - derives `foo_bar_baz` from `foo_bar.md` + `^baz`,
  - creates the project note,
  - seeds the new note's project completion task as `^prj`,
  - snapshots backlink rewrites before deleting the old task,
  - rewrites old block links through `rewriteBlockIdLinkOriginal(original, createdFile.basename)`,
  - deletes the source task only if rewrites succeed.
- The current rewrite helper is the narrow behavior to change:
  - wikilinks currently become `[[new_basename]]` with aliases/embeds preserved,
  - markdown links currently become `[text](new_basename.md)`.

## Design

Keep the command flow and failure semantics intact. Only change the replacement target produced for old task block-link
originals.

1. Add a small helper such as `getProjectTaskBlockLinkTarget(newBasename)` or inline equivalent logic:
   - wiki target: `${newBasename}^prj`, matching the requested `[[foo_bar_baz^prj]]` shape.
   - markdown target: `${newBasename}.md#^prj`, preserving markdown's existing URL-fragment style.
2. Update `rewriteBlockIdLinkOriginal(original, newBasename)`:
   - `[[target#^old]]` -> `[[new_basename^prj]]`
   - `[[target#^old|alias]]` -> `[[new_basename^prj|alias]]`
   - `![[target#^old]]` -> `![[new_basename^prj]]`
   - `[text](target.md#^old)` -> `[text](new_basename.md#^prj)`
3. Keep `collectBlockIdBacklinkRewrites()` unchanged unless inspection during implementation shows the existing cache
   source misses a supported old-link shape. The user request is about migrated target format, not backlink collection.
4. Keep `applyBlockIdLinkRewrites()` unchanged apart from calling the updated helper. Link counts and failure behavior
   remain the same.
5. Keep `createProjectNoteFromTask()` unchanged. The command should still delete the old task only after the new project
   note is seeded and all backlink rewrites succeed.
6. Update exported helpers only if a new helper is useful for focused validation.

## Validation

Static checks:

```bash
node --check /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js
```

Focused Node helper checks, using the plugin's existing `module.exports.helpers` pattern with Obsidian APIs stubbed:

- `rewriteBlockIdLinkOriginal("[[foo_bar#^baz]]", "foo_bar_baz")` returns `[[foo_bar_baz^prj]]`.
- Alias preservation: `[[foo_bar#^baz|daily task]]` returns `[[foo_bar_baz^prj|daily task]]`.
- Embed preservation: `![[foo_bar#^baz]]` returns `![[foo_bar_baz^prj]]`.
- Same-file old block link: `[[#^baz]]` returns `[[foo_bar_baz^prj]]`.
- Markdown link support: `[task](foo_bar.md#^baz)` returns `[task](foo_bar_baz.md#^prj)`.
- Unknown/unparseable originals still return `null`.

Manual acceptance after reloading Obsidian:

- In `foo_bar.md`, promote an open `#task ... ^baz` that is linked from another note.
- Confirm `foo_bar_baz.md` is created and its main project task still ends in `^prj`.
- Confirm old backlinks now read `[[foo_bar_baz^prj]]` or `[[foo_bar_baz^prj|alias]]`.
- Confirm the original task is removed only after rewrites succeed.
- Confirm no-block-ID project-note creation is unchanged.

## Risks / Notes

- The live vault is under Obsidian Sync and already dirty. Re-check `git -C /home/bryan/bob status --short` immediately
  before editing.
- The requested target uses bare block syntax, `[[note^prj]]`, rather than Obsidian's more common `[[note#^prj]]`.
  Existing local plugin code already treats bare `^block` as a block subpath in some contexts, so implement the
  requested format exactly.
- Because the plugin file already has unrelated uncommitted changes, inspect the final diff carefully. If committing is
  required before termination, stage only this task's hunk and leave the pre-existing dash-task changes untouched.

## Finish Criteria

- This plan is submitted with:

  ```bash
  sase plan sase_plan_project_task_links_to_prj.md
  ```

- Implementation changes, if made after plan submission, are limited to
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
