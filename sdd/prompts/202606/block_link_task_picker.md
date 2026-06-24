---
plan: sdd/tales/202606/block_link_task_picker.md
---
 Can you help me add a new functionality to Obsidian that triggers when I use the `^` character after an existing `^` character in a block link that the user is typing out?

- When this is triggered, we should show the user a nice menu of all of the open Obsidian tasks in the file that the block link is targeting.
- The selected task should be used to complete the block link.
- If the selected task does not already have a block ID, the user should be prompted to provide one.
- For example, as the user types the last `^` in `[[foobar^^]]`, this menu would trigger and prompt the user to select one of the open tasks in the ~/bob/foobar.md note file.
- I want you to lead the design on this one. Make sure you design this feature so it is intuitive, reliable, and (last but not least) beautiful!

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
 