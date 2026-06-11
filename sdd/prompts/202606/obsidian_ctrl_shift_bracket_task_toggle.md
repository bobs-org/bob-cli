---
plan: sdd/tales/202606/obsidian_ctrl_shift_bracket_task_toggle.md
---
 We already have a `<ctrl+]>` keymap that toggles an Obsidian note bullet to/from a checkbox. Can you help me
add a new `<ctrl+shift+]>` keymap that toggles whether the current bullet/checkbox is a proper Obsidian task or not?

- When invoked on a proper Obsidian task (i.e. the line was a checkbox and had a `#task` tag), the `#task` tag should be
  removed and any Obsidian task properties (e.g. `created`, `completion`, etc...) should be removed. The checkbox should
  also be converted to a normal bullet (like the `ctrl+]` keymap does).
- When the current checkbox / bullet was NOT a proper Obsidian task, we should make it one by first converting the
  bullet to a checkbox (if necessary). We should then prepend `#task ` and append ` [created::YYYY-mm-dd]` (use the
  current date) to the bullet item/line appropriately.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
