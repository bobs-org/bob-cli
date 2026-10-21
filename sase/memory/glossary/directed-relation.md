---
aliases:
  - labelled relation
  - directed task relation
---
# Directed Relation

*Requested · project*

A [[task-relation]] with a label, read from its source: "S ‹label› T", such as
`follow-up of`. It is stored on S's LINKS line. T shows Bob's inverse phrase when one
exists, otherwise "S · ‹label› → this". Capture `~T|label` makes the captured or owner
task the source, and `~~T|label` makes T the source. Clearing the label turns it back
into an [[undirected-relation]]. Two tasks carry at most one directed relation in each
direction.
