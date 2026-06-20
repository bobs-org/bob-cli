---
plan: sdd/tales/202606/split_wiki_link_markers.md
---
 We currently have two different behaviors for when the `^` character is typed after an Obsidian link's last `]` character:

1. If the link is an Obsidian block link, then we open up a prompt to rename the block (i.e. change the block ID).
2. Otherwise, we delete the link alias (if any), position the cursor before the first `]` in the link, and insert a `^` character to trigger the block link completion menu.

Can you help me split this functionality by having `^` always have the 2nd behavior and the new similar functionality for the `@` character always have the 1st behavior? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
