---
plan: sdd/tales/202607/pomodoro_next_status_in_progress.md
---
 When the `<ctrl+enter>` keymap is used to close a pomodoro task in a daily file, we are supposed to mark any obsidian tasks corresponding with non-transcluded block links for this pomodoro task as in progress. This was working before we added the new next obsidian task status recently. If a task has that status when we use this keymap, then it does not properly get changed to an in-progress status. Can you help me diagnose the root cause of this issue and fix it? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
  