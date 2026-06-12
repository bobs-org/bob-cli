---
plan: sdd/tales/202606/projects_sync_subprojects_1.md
---
 Can you help me start considering sub-projects when adding/removing the `p` property of the ^prj project task? Namely any project with sub-projects doesn't need to have tasks contained in it. A project contains sub-projects if any other project file links to it as its parent. See the `bob projects sync` command for context on all of this.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.


### Additional Requirements

- In order to be considered, any sub-project must have an open (not complete or cancelled) main ^prj task.