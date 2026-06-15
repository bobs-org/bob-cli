---
plan: sdd/tales/202606/revert_embed_fallback_add_ctrl_keymaps.md
---
 #fork:research.i.final.f1 Revert these changes.

- Instead, let's just add a new `<ctrl+shift+\>` keymap that does the same thing as the `\\` vim-mode normal mode
  keymap.
- Also, let's add a new `<ctrl+shift+=>` keymap that does the same thing as the current `\|` vim-mode normal mode
  keymap.
- Remove the `\\` and `\|` vim-mode normal mode keymaps so the same functionality is not defined by two different
  keymaps (which makes it harder to remember either of them).
- This solves the biggest pain point (the reliability

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
