---
plan: sdd/epics/202606/dataview_native_parity.md
---
 #fork:research.cdx-5 it's most important for the native engine to have full parity with the obsidian engine since agents will normally be running on a machine that does not have obsidian running. Can you help me implement all of the functionality needed to be able to support all of the same queries with the native engine that we do with the Obsidian engine currently? Once this is done, you should also delete the dynomark integration since we shouldn't need that anymore. 

This is a large piece of work that should be split into phases. I'll let you decide how many phases to create, but
keep in mind that each phase will be completed by a distinct agent instance (i.e. a distinct `claude` / `gemini` /
`codex` / `qwen` / `opencode` command). Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.

