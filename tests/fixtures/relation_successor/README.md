# Relation-successor vectors

`vectors.json` is the source of truth for successor-label behavior in
bob-cli and bob-plugins (`docs/task-relations.md` §20 and
`docs/task-dependencies.md` §§12.18–12.19). bob-plugins copies this file
byte-for-byte. Edit bob-cli first; never hand-edit the copy.

`base` supplies the canonical Ready source, its `starts after` edge, the
parked Next target, today's before/after ledger text, event, and enabled
config. A rule vector's `input` overlays those values; `cases` contains
separate subcases where one SA row names several independent outcomes.
The sections are:

| Section | Pins |
| --- | --- |
| `config` | YAML successor-label parsing, normalization, duplicate handling, overlaps, and warning counts |
| `key` | Configured label effect and display-inverse lookup, including `starts before` as hint-only |
| `rule` | Event and source state → ordered relation groups and held rows |
| `copy` | Bob-authored strings, pluralization, and text truncation |
| `ui_only` | SA cases owned by an interface, live-index wiring, Obsidian unlink, or undo test |

Each vector carries an SA or SU id that points to the matching contract
row. When a behavior changes, update the contract and vectors in the
same bob-cli change, then copy the file byte-for-byte to bob-plugins.
