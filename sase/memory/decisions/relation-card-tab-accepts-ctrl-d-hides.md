---
keyword: Relation Card Tab Accepts Ctrl-D Hides
aliases:
  - relation card keys
  - relation chooser
summary:
  "Ctrl+N/P navigate the relation card without touching the query, Tab accepts
  or completes the top match, selection is by key, and Ctrl+D hides what
  Return would insert."
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** The Bob Mac Capture relation card and the Obsidian
relation-label stage.

**Claim.** Card state is `query`, `selectedKey` (a label key, never an array
index), and the catalog fetched once on open. Navigation changes only
`selectedKey` and the sentence pill: it never edits the query, spawns a
process, or clears freshness. Typing clears the selection and refreshes only
the typed-row validation probe; the list ranks locally from the catalog. Tab
accepts the selected row, or completes the top match when nothing is
selected. One rule ties the keys together: Ctrl+D hides what Return would
insert (a ghost row with undo on a selection, the one-off toggle otherwise).
Ctrl+H shows or hides the Hidden section; selection is by key so a refresh
keeps the key when it survives and clears it when it disappears.

**Why.** Writing the chosen row back into the field and refetching collapsed
the list to one typed row, so Tab computed `(0+1) mod 1 = 0` forever and fast
Tabs walked a stale list; the index also outlived its list, started at 0
(skipping the most-used label), and was never drawn. Index-based selection
cannot survive a reorder; key-based selection can. Rejected alternatives:

- **Tab cycles suggestions** — the reported bug: refetch-per-accept makes
  every Tab a no-op on a one-row list.
- **Index-based selection** — a reorder silently moves the highlight to a
  different row.
- **Fetch per keystroke for the list** — navigation must be local and instant;
  only the typed-row probe may call bob.
- **Ctrl+Shift+H for the Hidden section** — the accepted `hidden_key`
  branch is Ctrl+H, mirroring `bob relations -H`; Ctrl+H claims
  backward-delete only inside this one-line card.

Evidence: `plan:202610/relation_label_vocabulary.md` RV5 with
`hidden_key = ctrl_h`; `research:202610/relation_card_label_vocabulary`;
`docs/task-relations.md` §16.

**Cost.** Ctrl+H loses its Emacs backward-delete meaning inside the one-line
card field (Backspace and fn-Delete still delete). Footers must teach the
keys because Tab no longer cycles.

**Reopens when.** The Hidden section needs a different chord, navigation must
refetch, or selection must survive a key disappearing.
