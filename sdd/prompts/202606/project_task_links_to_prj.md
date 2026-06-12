---
plan: sdd/tales/202606/project_task_links_to_prj.md
---
 When the `<ctrl+shift+alt+n>` Obsidian keymap is used, we promote the selected Obsidian task to a project note file. Assuming the task was in the `~/bob/foo_bar.md` file to start and had the `^baz` block ID, this keymap would automatically name the project note file `~/bob/foo_bar_baz.md`. Any block links to the previous task are converted to `[[foo_Bar_baz]]`. Can you help me instead migrate those block links to `[[foo_bar_baz^prj]]`, which should link to the main project task file in the project file note?

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
