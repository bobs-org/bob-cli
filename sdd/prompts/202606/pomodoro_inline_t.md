---
plan: sdd/tales/202606/pomodoro_inline_t.md
---
 #fork:12.f1.cdx Can you help me implement this?

- You should remove the `bob pomodoro-runtimes` command, which will no longer be used.
- The `se` Obsidian snippet that expands when the user presses `<tab>` should be updated to add the `t` property (let's
  use this instead of duration) inside the ledger parentheses (in the same place that the custom duration text would be
  placed now) instead of the current custom duration text (ex: "⏱️ 25m").
- The `~/bob/_templates/daily.md` file should be updated to include the inline query in the "Pomodoros" section title.
- The `\p` and `\P` vim-mode normal mode keymaps in Obsidian should be updated to modify the `t` property for that task
  line accordingly.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
