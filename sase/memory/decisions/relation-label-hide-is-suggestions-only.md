---
keyword: Relation Label Hide Is Suggestions-Only
aliases:
  - relation hide
  - relation unhide
  - one-off label
summary:
  "Hiding a relation label removes it from suggestions only; LINKS text,
  mirrors, chips, counts, and typed use stay unchanged, and using it never
  unhides it."
metadata:
  status: accepted
  decided: 2026-10-24
---

**Applies to.** bob-cli `bob relations hide` / `unhide`, capture-complete
`relation_label` rows, the Mac relation card, and the Obsidian relation-label
stage.

**Claim.** Hiding a relation label removes it from **suggestions** only: card
rows, the Obsidian stage, `relations list` defaults, and shell completion.
LINKS text, LINKED FROM mirrors, chips, counts, and typed use are unchanged.
Typing a hidden label's exact name still works and is flagged `hidden`; using
it never unhides it. A one-off `||` label hides only after the capture's notes
commit succeeds, and hiding a visible existing label warns with the link count
instead of blocking. This is the twin of
[[decisions:theme-hide-is-recent-only]].

**Why.** Deleting a hidden label from the catalog would rewrite history and
lose its count. Auto-unhiding on use would fight the hide gesture the next
time the label is typed. Blocking typed use of a hidden label would make old
notes un-linkable. Auto-hiding anything (for example low-count labels) would
create silent state Bryan never asked for. Rejected alternatives:

- **Delete from the catalog** — Stats-equivalent counts and old LINKS lines
  would lose the name.
- **Unhide on next use** — typing a hidden label once would bounce it back
  into every suggestion list.
- **Block inserting hidden labels** — existing notes carrying the label could
  no longer be linked with it.
- **Auto-hide** — any silent hide breaks the no-silent-state rule; every hide
  is an explicit Ctrl+D, `bob relations hide`, or `||`.

Evidence: `plan:202610/relation_label_vocabulary.md` RV4;
`research:202610/relation_card_label_vocabulary`;
`docs/task-relations.md` §16; commits `57d77612`, `6def3ac8`, `895b9ba7`.

**Cost.** Hidden labels still occupy the catalog and still wear counts. Clients
must filter suggestions after ranking, not drop the catalog row, and must
render bob's warning message instead of computing the count.

**Reopens when.** Hide should also suppress counts or mirrors, typed use of a
hidden label should refuse, or using a hidden label should be an unhide
gesture.
