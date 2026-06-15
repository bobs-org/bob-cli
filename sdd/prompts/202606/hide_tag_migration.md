---
plan: sdd/tales/202606/hide_tag_migration.md
---
 Can you help me migrate the `[p::2]` property used by main project (`^prj`) and main reference (`^ref`) note file tasks that have special support included in the `bob projects` and `bob highlights` commands to a new `#hide` tag?

- Make sure to update all existing main project/reference note tasks in my ~/bob/ Obsidian vault to use this new tag instead of `p`.
- Make sure to update the tasks query in the `~/bob/_templates/daily.md` template (as well as today's daily file) accordingly. Namely, we should remove all references to the `p` property from that query and exclude tasks that have the `#hide` tag instead.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
