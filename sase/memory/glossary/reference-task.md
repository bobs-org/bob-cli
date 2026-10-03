---
keyword: Reference Task
aliases:
  - "ref task"
---

The single source-level `#task` checkbox in a [[reference-note]], identified by the
exact trailing block ID `^ref`, that tracks progress through the external material. In
Highlights-generated notes it links to the PDF: `[ ]` means `ready`, `[*]` means `next`,
`[/]` means `wip`, `[x]`/`[X]` means `read`, and `[-]` means `abandoned`. Highlights
sync reconciles this with note and PDF status. New lines include `#ref`; legacy lines
without that tag remain recognized. Follow-up tasks track separate work. See
`docs/highlights-ref-sync.md`.
