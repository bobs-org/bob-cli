---
plan: sdd/tales/202606/highlight_task_completion_order.md
---
 #fork:3p.f1.f1.f1.f1.f1 It seems like now if I add a few task notes to a PDF and then mark the reference note `^task` as complete, the `bob highlights` command will update the frontmatter `status` field and the PDF marker note to "read" and then not process any of the tasks. This is NOT correct. The task processing should occur before we mark the status as "read" (this way we create any new tasks the user added before finishing reading the PDF, but never check that PDF again in future runs for task notes). Can you help me fix this? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
