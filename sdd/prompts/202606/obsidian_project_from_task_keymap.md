---
plan: sdd/tales/202606/obsidian_project_from_task_keymap.md
---
  We already have an Obsidian `<ctrl+shift+n>` keymap that creates a new project file using the
`~/bob/_templates/new_project.md` template. Can you help me create a new `<ctrl+alt+shift+n>` that does the same thing,
but uses the currently seleected Obsidian task to construct the `^prj` task's in place of the
`(REPLACE WITH PROJECT COMPLETION CRITERIA)` placeholder text that we currently use? After the new project file has been
created:

- The original task should be deleted.
- The new project file should be opened in the current tab.
- A useful toast should be displayed to the user.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
