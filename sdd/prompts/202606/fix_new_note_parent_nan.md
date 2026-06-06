---
plan: sdd/tales/202606/fix_new_note_parent_nan.md
---
 When I use the `<enter>` keymap in Obsidian on a link that points to a file that doesn't exist, we create the file using the ~/bob/_templates/new_note.md template. But the `parent` field, which is supposed to link back to the file we just jumped from, always seems to have a value of "NaN". Can you help me diagnose the root cause of this issue and fix it? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
 