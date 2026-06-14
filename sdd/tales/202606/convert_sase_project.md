---
create_time: 2026-06-14 08:32:55
status: wip
prompt: sdd/prompts/202606/convert_sase_project.md
---
# Plan: Convert `~/bob/sase.md` Into a Bob Project File

## Context

`~/bob/sase.md` is currently an area note with frontmatter `type: "[[area]]"` and a `## Tasks` section. It already has
local, uncommitted vault changes: `parent` was changed to `[[dev]]`, `Related: [[ai]]` was added, and two June 14 tasks
were added. Treat the current file contents as authoritative and preserve those changes while converting the note.

The Bob project convention, per `~/bob/_templates/new_project.md`, `~/bob/sase_install.md`, and `docs/projects.md`, is:

- frontmatter marks the note as `type: "[[project]]"` and usually includes `status`, `created`, `task_count`, and
  `open_task_count`;
- the first body item is exactly one project completion task ending in `^prj`;
- active parent projects with open child projects carry one machine-compatible sub-project line nested under `^prj`;
- normal project work stays under `## Tasks`;
- notes and non-action backlog material can live under `## Project Notes`.

## Proposed Shape

Update `~/bob/sase.md` in place:

1. Keep `parent: "[[dev]]"`.
2. Change `type: "[[area]]"` to `type: "[[project]]"`.
3. Add `status: wip`.
4. Add `created: 2026-06-01T03:30:02-04:00`, using the oldest git history timestamp for this note as the best available
   creation value.
5. Keep `done_tasks: "[[done/sase_done]]"` because at least one existing project note also uses `done_tasks`, so
   dropping it is not necessary for project validity.
6. Set `task_count: 7` and `open_task_count: 7`, because after moving the P1 backlog items there should be seven open
   non-`^prj` project tasks left.
7. Insert a root project completion task immediately after frontmatter, for example:

   ```markdown
   - [ ] #task SASE is ready for regular use as a structured agentic software engineering system! [p::2] ^prj
     - 🧩 **Sub-projects:** [[sase_blog]] • [[sase_install]] • [[sase_piw_multi_prompt]]
   ```

   The child project list is based on direct open project notes whose `parent` links resolve to `sase`.

8. Preserve the existing H1 title after the root project task.
9. Move the current top-level context bullets (`SASE Blog Post Drafts`, `Related`) into `## Project Notes` so the body
   follows the normal project-note flow: root task, title, `## Tasks`, `## Project Notes`.

## Task Migration

In the existing `## Tasks` section, move every top-level task containing `[p::1]` or `[p:: 1]` into a bottom
`### Future Work` subsection under `## Project Notes`, matching the style used in `~/bob/bob_projects.md`.

For each moved item:

- convert it from a task to a normal bullet by removing checkbox syntax and `#task`;
- remove only the priority field (`[p::1]` / `[p:: 1]`);
- preserve useful inline context such as `created`, `dependsOn`, `id`, wiki links, and block IDs so backlinks and
  chronology are not lost;
- move child bullets with their parent and convert any nested checkbox-only backlog item into a normal nested bullet
  too.

The remaining `## Tasks` section should contain only the seven unprioritized open tasks currently on lines 32-34 and 36
and 39-41 of `sase.md`.

## Verification

After editing:

1. Run `rg -n '\[p::\s*1\]' ~/bob/sase.md` and confirm it returns no matches.
2. Run `bob projects list -b ~/bob` and inspect the `sase` row for `type: project`, one valid open `^prj`, and expected
   task counts.
3. Run `bob projects sync --dry-run -b ~/bob` to check whether the converted `sase.md` would be rewritten. Do not run
   non-dry-run sync unless the dry run shows only the intended `sase.md` normalization or the user explicitly wants
   broad vault sync changes.
4. Review `git -C ~/bob diff -- sase.md` to confirm the diff is limited to the requested project conversion and P1
   backlog move.

## Risks and Guardrails

- The vault is already dirty, including `sase.md`; do not reset or discard any pre-existing user edits.
- Do not modify memory files.
- Do not update generated files under `~/bob/_generated`.
- Avoid using `bob projects sync` in mutating mode by default because it can legitimately touch other project notes in
  the dirty vault.
