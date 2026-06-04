---
plan: sdd/tales/202606/highlights_ref_cancelled_task.md
---
 #fork:2e I'm seeing the following error on another machine when running the `bob highlights scan` command. I'm not sure if this has anything to do with the changes that we just made, but I wouldn't be suprised.  Can you help me diagnose the root cause of this issue and fix it? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.

```
bob highlights: scan completed with 1 per-PDF failure(s)
planning failures:
  /Users/bbugyi/bob/lib/chat/bulk_obsidian_task_properties.pdf: generated PDF task line on line 4 is malformed; expected '- [ ] #task [[...pdf]] [p::2] ^task' or '- [x] #task [[...pdf]] [p::2] ^task'
```