---
keyword: Prose Note
summary: "A Bob note for author-owned free-form Markdown; its primary role is prose."
aliases:
  - prose note
  - article note
---

# Prose Note

A Bob vault note whose body is free-form authored Markdown that the author owns:
paragraphs, headings, lists, tables, quotes, and code. bob may format its layout and
(unless turned off) section numbers, but never changes a word or adds structure. Its
membership test is the note's primary role: an area or project with prose sections
stays an area or project. A prose note holds no `#task` lines, `^prj` or `^ref`
anchors, or second type.

The canonical frontmatter spelling is `type: "[[prose]]"`. Legacy `type: "[[article]]"`
notes remain prose notes, and `bob articles` remains a permanent CLI alias. See
`docs/prose.md`.

Not a [[notebook]], a [[record-note]], a [[reference-note]], a [[task-drawer]], or an
[[annotation-section]]. A [[project-note]] or [[area-note]] with prose sections stays
one.
