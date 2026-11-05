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

Landing into an existing open [[pomodoro]] merges instead of retyping: when a
[[task-link]] lands in an entry of another type, the destination gains ` + TYPE` and
no task field is written. The Obsidian link, move, and successor pickers merge on
Enter and retype only on Alt+Enter. Creating a new entry from a typed or picked name
retypes the task landing in it, so the new name already agrees with its link; a
container name types nothing.

[[capture]] `=*` merges planned entries into a survivor, `=/` splits merged-in
types out into new planned entries below the source, and Obsidian Ctrl+S splits the
selected entry the same way: two types split immediately, three or more prompt for
the types to move.

At close, `settle_closed` names the closed session from the labels plus exactly the
types of its worked links, in entry order. It appends missing worked types in catalog
spelling and substitutes in place when one type is removed and one is added;
single-type names can rename. The continuation first sheds unsupported merged types,
keeping the base when needed, then agrees with its carried links so it is an
`agree_entry` fixed point. A name that normalizes to MEET is never merged, split,
settled, shed, or renamed.

This is not a container, not a Pomodoro home, and not a D6 rename.
