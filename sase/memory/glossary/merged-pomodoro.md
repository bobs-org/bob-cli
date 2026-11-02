---
keyword: Merged Pomodoro
aliases:
  - merged pomodoro
  - composite pomodoro
  - base type
---

A [[pomodoro]] whose ` — NAME` holds two or more [[task-type]] components, for
example `BUILD + FIX + READ`. The first component is the base type, the one the
others were merged into; the rest are merged-in types, in name order. With a
configured vocabulary, non-type components are labels and never count; without one,
each component is a type unless it is an inventory label. `BOB + BUILD` is not merged
when BOB is a label, and a container such as `LATER` is not merged.

Landing merges instead of retyping: when a [[task-link]] lands in an open [[pomodoro]]
of another type, the destination gains ` + TYPE` and no task field is written. The
Obsidian link, move, and successor pickers merge on Enter and retype only on
Alt+Enter; naming an entry still retypes its linked tasks.

[[capture]] `=*` merges planned entries into a survivor, `=/` splits merged-in
types out into new planned entries below the source, and Obsidian Ctrl+S splits the
selected entry the same way: two types split immediately, three or more prompt for
the types to move.

At close, the closed session narrows to the labels plus exactly the type components
with worked links, in entry order; the created continuation sheds each type with no
carried link, keeping the base when everything would go. Single-type entries never
rename, and an entry whose name normalizes to MEET is never merged, split, narrowed,
or shed.

This is not a container, not a Pomodoro home, and not a D6 rename.
