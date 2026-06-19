---
plan: sdd/tales/202606/capture_bullets.md
---
 Can you help me add support to the `bob capture` command for adding bullets instead of Obsidian tasks (which is all it currently supports)?

- When the submitted text ends with a `#<X>`, then we use this as an indication that a bullet should be created instead of a task.
- Bullets should still have a `[created::YYYY-mm-dd]` property added to them.
- Bullets should NOT be added to the "Tasks" section. Instead, they should be added to the first section found that starts with `<X>` (that is not the "Tasks" section) or to the zeroth section (no section) otherwise.
- Bullets should be added to the target section after the section header and a single blank line, if this is the first bullet in that section; otherwise, the new bullet should be placed on the line after the very last bullet in that section.
- We should support the case where `<X>` is not provided (i.e. the user just inputs a `#` at the end of the text instead of `#<X>`). In this case, we should use the first non-"Tasks" section we find.
- The `@<note_file_name>` syntax is also supported at the end of the text. `#<X>` and `@<note_file_name>` should be able to be used in any order at the end of the text (e.g. `Some bullet. @foo #` is equivalent to `Some bullet. # @foo`.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
