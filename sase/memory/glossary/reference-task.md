---
keyword: Reference Task
aliases:
  - "ref task"
---

The single source-level `#task` checkbox that tracks progress through the external
material of a [[reference-note]]. A modern reading task lives in the reference's
parent (area, project, or inbox) note, identified by the trailing block ID
`^ref-<slug>` — with a `-vN` suffix for rereads (`^ref-<root-slug>-vN`, alias
`Title (vN)`) — while the reference note itself carries a managed embed pointing at
it. A legacy in-note `^ref` tracker is still recognized. In Highlights-generated
notes the task links to the PDF: `[ ]` means `ready`, `[*]` means `next`, `[/]` means
`wip`, `[x]`/`[X]` means `read`, and `[-]` means `abandoned`. Highlights sync
reconciles this with note and PDF status. New lines include `#ref`; legacy lines
without that tag remain recognized. Follow-up tasks track separate work. See
`docs/highlights-ref-sync.md`.
