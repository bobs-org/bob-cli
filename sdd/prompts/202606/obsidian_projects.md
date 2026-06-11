---
plan: sdd/epics/202606/obsidian_projects.md
---
 Can you help me add support for the concept of "projects" to my Obsidian vault? Projects should be defined as note files
that have the `type: [[project]]` frontmatter property.

This is a large piece of work that should be split into phases. I'll let you decide how many phases to create, but
keep in mind that each phase will be completed by a distinct agent instance (i.e. a distinct `claude` / `gemini` /
`codex` / `qwen` / `opencode` command). Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.



## Requirements

### New `~/bob/projects.base` File

- You should create a new `~/bob/projects.base` file that shows a table of my active/waiting projects with useful data
  included. Make sure you include counts of how many tasks are defined for each project (i.e. how many Obsidian tasks
  are defined in the project file's "Tasks" section). I want you to lead the design on this one. Just make sure it looks beautiful!
- The “Tasks”, “Reading List”, and "Projects" (which is new and should contain a transcluded view of the
  ~/bob/projects.base file) sections should be migrated from the `~/bob/_templates/daily.md` file to a new
  `~/bob/dash.md` file (update today's daily file too)!

### Migrating Existing Project Files

- You will need to create the new `~/bob/area.mda` file I think. Use your best judgement (this is the GTD area of
  responsibility note file that area note files will link to as their `type`).
- With regards to existing project files (i.e. files that alredy have a `type` of `[[project]]`): the `~/bob/job.md`
  file should have a `type` of `[[area]]` and the `~/bob/obsidian.md` file shouldn't be a project at all. Make
  `~/bob/bob.md` a project in its place.
- ALL note files that contain open Obsidian tasks should be converted to either a type `[[area]]` or type `[[project]]`
  file, so use your best judgement for the rest of them.

### Project Note File Template and the NEW `<ctrl+shift+n>` Keymap

- **Question**: How do project files get created with the right structure?
  - **Answer**: The user will use a new `<ctrl+shift+n>` keymap that works like the built-in `cmd+n` but using a new
    `~/bob/_templates/new_project.md` template instead of the `~/bob/_templates/new_note.md` template. This template
    should contain a "## Tasks" section and a "## Project Support" section.
- The following frontmatter properties should be supported by project note files:
  - `parent: <area_file | project_file>`
  - `type: [[project]]`
  - `priority: <N>` (optional--defaults to 0)
  - `status: "wip"|"waiting"|"done"|"canceled"` (defaults to "wip" for new project files)
- The `parent` property should default to a link to the note that created it, which MUST be of type `[[area]]` or
  `[[project]]`; otherwise, the `<ctrl+shift+n>` keymap should throw an error (via a toast) to the user.
- A project note's `priority: <N>` property is optional, but should support integer values (starting at 0).