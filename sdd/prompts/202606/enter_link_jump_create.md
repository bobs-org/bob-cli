---
plan: sdd/tales/202606/enter_link_jump_create.md
---
  Can you help me add robust support for link jumping and creation via the ~/bob/_templates/new_note.md template when the user hits enter.

- Counts should be supported (e.g. `5<enter>` might trigger jumping to the file associated with the only link on  the line five lines below the current line). 
- If there are multiple links on the target line, the user should be prompted to select which one to create and/or jump to. Use the same interface that we use for selecting child note files, which supports filtering.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
