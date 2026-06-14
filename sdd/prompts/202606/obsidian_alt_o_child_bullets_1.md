---
plan: sdd/tales/202606/obsidian_alt_o_child_bullets_1.md
---
 Can you help me add new `<alt+o>` and `<alt+O>` (aka `<alt+shift+o>`) keymaps to Obsidian that work like the vim-mode `o` / `O` keymaps except for they always prefill the next/previous line with a `-  ` which is indented appropriately (2 spaces per level) depending on the current lines indentation such that the bullet that we generate is one level of indentation beyond the current line? For example, for the lines `foo bar baz` or `- foo bar baz`, the `<alt+o>` keymap should insert `  - ` on the next line, move the cursor to the space after the `-`, and then switch to insert mode.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
