---
keyword: Album Note
---

A root-level Bob vault Markdown note with frontmatter `type: "[[album]]"` that owns
one record-note schema and its record folder. The note stem is the folder name and the
capture route. Its `parent` names an area or project, and its `summary` describes the
collection. The nested `record:` mapping defines the record noun, label, date policy,
and ordered fields. `bob albums sync` maintains the generated index and the owner's
machine-owned `## Albums` catalog. Albums are not notebooks, task types, task homes, or
root parents: parent and type walks pass through them to the area or project above.
Record notes are created and updated with `bob records new|update` or capture. See
`docs/records.md`.
