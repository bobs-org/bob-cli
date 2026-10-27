---
keyword: Reference Note
aliases:
  - "ref note"
---

A Bob vault Markdown file with frontmatter `type: "[[ref]]"` for a particular piece of
external reference material, such as a book, article, or captured conversation. It
gathers source links plus notes or highlights. It holds no open tasks. It shows its
reading task (`#task #ref`, living in the parent area, project, or inbox note's
`## Tasks`) through the managed embed `![[<residence>#^<id>]]` one blank line below the
H1. Frontmatter `parent` is projected from where that task lives (a `done/` archive maps
to that file's `parent:`). Annotation follow-up tasks default to the parent note. A
frozen in-note `^ref` tracker is the legacy v1 shape. Highlights sync generates this
form; legacy notes may lack the task. See `docs/highlights-ref-sync.md` and
[[reference-task]].
