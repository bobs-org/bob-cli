---
plan: sdd/tales/202606/obsidian_depends_on_local_task_picker_1.md
---
 Can you help me add support for the `dependsOn` property to the `<ctrl+shift+p>` keymap?

- We will need to add a new `dependsOn` property to the ~/.config/bob/config.yml file (modify the version in my chezmoi repo).
- This property should have a value of `local_task_id`, which is a new type of value that indicates that the property should have a value equal to the block ID of a local task.
- When the user selects this property from the menu that is triggered by the `<ctrl+shift+p>` keymap, a new menu should pop up containing options for every open Obsidian task in the current file.
- If the selected tasks does not already have a block ID, the user should be prompted to provide one.
- See what happens when the special `^^` functionality is triggered in Obsidian for inspiration.
- I want you to lead the design on this one. Make sure you design this feature so it is intuitive, reliable, and (last but not least) beautiful! 

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
 

### Additional Requirements

- We should (e.g. in order to work with Obsidian tasks queries) also add an `[id::<block_id>]` property to the target Obsidian task if that property does not already exist on that task.