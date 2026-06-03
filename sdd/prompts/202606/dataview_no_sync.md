---
plan: sdd/tales/202606/dataview_no_sync.md
---
 The `bob dataview` command currently runs the `on sync` command before running its query. This is not correct. This command should not run `ob sync`. We have a background service for that. If we were writing to obsidian, it would be different but we're just reading from it. Can you help me fix this? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
