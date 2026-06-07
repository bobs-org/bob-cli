---
create_time: 2026-06-07 09:01:56
status: done
prompt: sdd/prompts/202606/new_note_current_parent_fallback.md
---
# Plan: Try Current Note Parent For Cmd+N New Notes

## Goal

Update the Bob vault's `new_note.md` template behavior so a note created through the current Cmd+N/root-folder-template
path tries to use the note that was open when the key map was triggered as `parent`, when that source note is available.

If Templater cannot identify a valid source note, the rendered `parent` should fall back to `[[org]]`.

Keep the existing behavior for explicit Templater note creation, including the missing-link/Enter flow: if Templater
exposes a valid source note through `tp.config.active_file`, use that source note as the parent.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read required Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault workflow context before planning changes to the new-note template behavior"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits require a status check first, preserving unrelated dirty files, and
  committing only task-related vault changes with the SASE git commit workflow before finishing.
- Read the prior plans:
  - `sdd/tales/202606/obsidian_cmd_n_new_note_template.md`
  - `sdd/tales/202606/fix_new_note_parent_nan.md`
  - `sdd/tales/202606/new_note_org_default_parent.md`
  - `sdd/tales/202606/new_note_created_field.md`
- Inspected the live template: `/home/bryan/bob/_templates/new_note.md`
- Inspected Templater settings: `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
- Inspected the installed Templater 2.20.5 implementation:
  `/home/bryan/bob/.obsidian/plugins/templater-obsidian/main.js`
- Inspected the Obsidian hotkey/settings files enough to confirm there is no explicit `Mod+N` override in
  `hotkeys.json`; the current Cmd+N path is still Obsidian's built-in new-note command followed by Templater's
  file-creation trigger.
- No new `bob-cli` CLI subcommands or options are being added, so `memory/long/cli_rules.md` is not required.

## Current Behavior

The current template frontmatter is:

```md
parent: "<% (() => { const defaultParent = '[[org]]'; if (tp.config.run_mode === 2) return defaultParent; const
activePath = tp.config.active_file?.path; const lastOpenPath = tp.app.workspace.getLastOpenFiles()[0]; const path =
activePath || lastOpenPath; return path && tp.file.find_tfile(path) ? `[[${path}]]` : defaultParent; })() %>" created:
<% tp.file.creation_date("YYYY-MM-DD[T]HH:mm:ssZ") %>
```

That fixed the previous request by making Templater's folder-template/file-creation mode (`run_mode === 2`) return
`[[org]]` immediately.

The downside is that Cmd+N notes no longer even attempt to use the note that was active before the new file was created.

## Feasibility Analysis

This is possible to attempt from the template, but Templater does not provide a guaranteed "pre-create source file"
field for the built-in Cmd+N path.

Relevant local Templater details:

- Templater's `create_running_config(...)` sets `tp.config.active_file` from:

  ```js
  workspace.activeEditor?.file ?? workspace.getActiveFile();
  ```

- Explicit `create_new_note_from_template(...)` renders with `run_mode === 0` before Templater opens the new note, so
  `tp.config.active_file` should still be the source note.
- Folder-template/file-creation rendering uses `run_mode === 2`.
- The folder-template handler waits about 300 ms after the vault `create` event before writing the template. By then,
  Obsidian may have already made the newly created note active. In that case `tp.config.active_file` can be the target
  note, not the source note.

Given that timing, the template should not trust `active_file` blindly. It should:

1. Prefer `tp.config.active_file?.path` only when it is not the target file and resolves to a real file.
2. Then scan workspace recent-file history, if the runtime exposes `tp.app.workspace.getLastOpenFiles`.
3. Ignore the target file and the template file while scanning candidates.
4. Fall back to `[[org]]` if no valid candidate exists.

This provides best-effort source-note parenting without adding a custom command or plugin state.

## Primary Design

Update only `/home/bryan/bob/_templates/new_note.md`.

Replace the `parent` expression with a candidate-selection expression that works for all relevant Templater modes:

```js
(() => {
  const defaultParent = "[[org]]";
  const targetPath = tp.config.target_file?.path;
  const templatePath = tp.config.template_file?.path;
  const activePath = tp.config.active_file?.path;
  const historyPaths =
    typeof tp.app.workspace.getLastOpenFiles === "function" ? tp.app.workspace.getLastOpenFiles() : [];
  const candidates = [activePath, ...historyPaths];
  const path = candidates.find(
    (candidate) => candidate && candidate !== targetPath && candidate !== templatePath && tp.file.find_tfile(candidate),
  );
  return path ? `[[${path}]]` : defaultParent;
})();
```

The actual template should keep this as a single inline Templater expression in YAML frontmatter, matching the current
file style.

Important behavior changes:

- Remove the `if (tp.config.run_mode === 2) return defaultParent;` short-circuit.
- Continue to use `[[org]]` as the ultimate fallback.
- Avoid making the new note its own parent.
- Avoid making `_templates/new_note.md` the parent if the template itself somehow becomes active.
- Guard `getLastOpenFiles` so the template does not throw if that workspace method is unavailable in a future Obsidian
  runtime.
- Scan all recent file paths instead of only `[0]`, because the newly created target may be first in the recent-file
  list after Cmd+N.

## Scope

Expected implementation file:

- `/home/bryan/bob/_templates/new_note.md`

Expected files not to edit for the primary implementation:

- `/home/bryan/bob/.obsidian/plugins/templater-obsidian/data.json`
- `/home/bryan/bob/.obsidian/hotkeys.json`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- any `bob-cli` source files
- any memory files

## Fallback If Template-Only Recovery Fails

If live testing shows that neither `tp.config.active_file` nor workspace recent-file history exposes the pre-Cmd+N note,
the template-only approach cannot guarantee the exact triggering note.

The more robust fallback would be a separate, more invasive plan:

1. Add a custom command that captures the current active file path before creating the new note.
2. Bind that command to `Mod+N`.
3. Pass the captured source path to the template, or store it in a small temporary plugin state consumed by the
   template.
4. Unbind or override the built-in new-note command only if needed to avoid duplicate handling.

Do not implement that fallback as part of the primary change unless template-only testing proves it necessary and the
broader hotkey/plugin scope is explicitly accepted.

## Implementation Steps

1. Re-check vault status:

   ```bash
   git -C /home/bryan/bob status --short
   ```

2. Edit only the `parent` expression in `/home/bryan/bob/_templates/new_note.md`.
3. Preserve the existing `created` line and heading.
4. Inspect the diff and confirm it is limited to the template expression.

## Verification

Static checks:

```bash
git -C /home/bryan/bob diff --check -- _templates/new_note.md
git -C /home/bryan/bob diff -- _templates/new_note.md
```

Focused JavaScript harness cases:

- `run_mode === 2`, `active_file.path` is the old source note, `target_file.path` is the new note: returns the source
  note wikilink.
- `run_mode === 2`, `active_file.path` is the new target, `getLastOpenFiles()` returns `[target, source]`: skips target
  and returns the source note wikilink.
- `run_mode === 2`, active/history contain only the target or invalid paths: returns `[[org]]`.
- `run_mode === 0`, explicit Templater creation with valid `active_file.path`: returns the source note wikilink.
- `getLastOpenFiles` is missing or not a function: does not throw and falls back according to active/source
  availability.
- `active_file.path` equals `_templates/new_note.md`: does not use the template file as parent.

Manual/live acceptance after Obsidian or Templater reloads the template:

1. Open an ordinary note.
2. Press `Cmd+N`.
3. Confirm the new note is populated from `_templates/new_note.md`.
4. Confirm `parent` uses the ordinary note when Obsidian/Templater exposes it.
5. From a context where no source note is available, create a new note and confirm `parent: "[[org]]"`.
6. Create a missing-link note through the existing Enter/link workflow and confirm it still gets the source note parent.
7. Delete scratch notes created solely for testing.

## Risks

- The exact pre-Cmd+N note is not guaranteed by Templater's folder-template API. This plan makes a best-effort attempt
  and falls back cleanly.
- `getLastOpenFiles` is already used by the template history, but I did not find a local public type definition for it.
  The implementation should guard it.
- The root folder-template rule applies more broadly than Cmd+N. Any new empty Markdown file that uses
  `_templates/new_note.md` will now try the same source-note candidate logic before falling back to `[[org]]`.
- A live Obsidian smoke test is the only way to prove whether the recent-file ordering captures the exact note that was
  open when the key map was triggered.

## Finish Criteria

- This plan is submitted with:

  ```bash
  sase plan sase_plan_new_note_current_parent_fallback.md
  ```

- No vault implementation files are edited before plan submission.
- If implementation later edits under `~/bob`, commit only the task-related vault file(s) with the required
  `sase_git_commit` workflow, leaving unrelated dirty vault changes untouched.
