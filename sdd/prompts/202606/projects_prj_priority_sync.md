---
plan: sdd/tales/202606/projects_prj_priority_sync.md
---
 The `bob projects sync` command currently adds a `scheduled` property to the `^prj` task if no unprioritized
tasks exist in that project file. Can you help me stop using the `scheduled` property and instead just remove the
`[p::2]` property from that task (which will cause it to show up in the `~/bob/dash.md` file's "Tasks" section)? Also,
make sure that we add the `[p::2]` property to any `^prj` tasks that we find that exist in project files which contain
unprioritized (i.e. no `p` property) Obsidian tasks.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
 