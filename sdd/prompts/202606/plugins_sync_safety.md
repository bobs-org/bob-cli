---
plan: sdd/tales/202606/plugins_sync_safety.md
---
 Can you help me make the `bob plugins sync` command much safer to run?

- We should show a diff between each old file and each new file.
- We should also add a `-d|--dry-run` option that shows these same diffs.
- Also anytime we overwrite a plugin file in the Obsidian Vault, we should copy the file we are overwriting to a backup copy and make it clear via the command output where that backup copy file lives.

I want you to lead the design on this one. Make sure you design this feature so it is intuitive, reliable, and (last but not least) beautiful! Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
 