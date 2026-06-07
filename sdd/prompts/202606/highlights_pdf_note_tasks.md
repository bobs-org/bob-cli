---
plan: sdd/tales/202606/highlights_pdf_note_tasks.md
---
 Can you help me add support to the `bob highlights` command for triggering new Obsidian task creation based
on PDF notes notes?

- Tasks should be identified by looking for a bullet with the `#task` tag. Multiple tasks can be defined in one note.
- Tasks can be written by users in PDFs as highlights comments (these are rendered using the `[comment] ` prefix) or as
  sticky note comments.
- For each task bullet note in a PDF that is found, a new Obsidian task should be created in the reference note file
  corresponding with that PDF (under the `^task` task as a new Obsidian task, not a sub-bullet / sub-task).
- For example, `- #task Foo bar baz` should triggered the creation of a new
  `- [ ] #task Foo bar baz [created::2026-06-07]` line in the reference note file.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
