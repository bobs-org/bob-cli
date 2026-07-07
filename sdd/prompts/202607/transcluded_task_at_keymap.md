---
plan: sdd/tales/202607/transcluded_task_at_keymap.md
---
 We already have the `!` keymap in Obsidian, which toggles whether or not an Obsidian task associated with a block link on the current line is transcluded. Can you help me add a new `@` keymap, which should only be active when the cursor is on one of these lines and the block link is transcluded?

- This keymap should just toggle the task's state from open (i.e. `[ ]`) to in-progress (i.e. `[/]`).
- It's important that this keymap is only active when it should be because the `@` symbol is used to run macros in normal mode.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
 