---
keyword: Relation Equals Alias And Flips
aliases:
  - equals alias
  - relation direction flip
  - fast double-equals escape
summary:
  "`=` is the only relation alias for `~` under Bob's Links To gate; a glued
  `~`/`=` flips a complete relation token; `~`/`=` flip direction in the
  editor, picker, and label card; a fast `==` types one literal `=`."
metadata:
  status: accepted
  decided: 2026-11-07
---

**Applies to.** bob-cli and Bob Mac Capture.

**Claim.** `=` is the only relation alias for `~`, and it fires only where
Bob's completion grammar would open the Links To picker. The past-segment `-`
inside `x=` tokens stays. A `~` or `=` typed glued to a complete `~note:id` /
`~~note:id` flips its direction and consumes the trigger; after a `|label`,
only `~` flips, because `=` is valid label text. `~` / `=` flip Links To ↔
Linked From in the editor, in the relation picker (keeping the filter), and
in the relation label card (keeping the query and selection). A card flip is
an edit, not navigation: it clears stale candidates and verdicts and
re-fetches the catalog and verdicts for the new direction. A fast `==` (two
non-repeat `=` key-downs within 350 ms key-to-key, nothing in between) undoes
whatever the first `=` did and types one literal `=`. On an alias-opened
empty Links To, Space restores `= ` and Escape restores `=`.

**Why.** The consolidated research
(`research:202611/relation_equals_alias_and_direction_flips/relation_equals_alias_and_direction_flips.md`)
accepted all four gestures with its A1–A10 adjustments, and its
rejected-alternatives table ruled out the credible alternatives:

- **`<` / `>` direction markers and modifier chords** — rejected; `~` / `=`
  reuse keys already on the relation path.
- **Auto-inverting the label phrase on flip** — rejected; a card flip keeps
  the label phrase and never maps `supersedes` ↔ `superseded by`.
- **Keeping `-` as the relation alias** — rejected; `-` collides with pasted
  and typed prose after a space, so `=` takes the alias and `-` stays
  literal there.
- **Burst or timing-based syntax reaching the grammar** — rejected; pasted
  text, `bob capture`, and `capture-parse` stay literal, and Bob remains the
  only grammar authority.
- **The "just accepted" arm** — rejected for now; the opener arms
  Space/Escape restores instead.

Evidence: `plan:202611/relation_equals_alias_and_direction_flips.md`;
`0869f792 feat(capture)!: rewrite relation directions`; `docs/capture.md`
("Sigil alias", "Relation direction toggle"); `docs/task-relations.md`.

**Cost.** The documented `-` → `~` relation alias is gone (a `!` break: a new
Mac with an old Bob degrades to a literal `=`, and an old Mac with a new Bob
degrades to a literal `-`). The 350 ms key-to-key window is timing state on
every surface where `=` stands for `~`. The auto-inserted trailing space
after a picker accept stays, so at `~foo:bar ␣` a `=` starts a second
relation instead of flipping.

**Reopens when.** Bryan wants the "just accepted" arm instead of the
Space/Escape restores, or a different `==` result than one literal `=`.

## Linked References

1. [[decisions:relation-card-tab-accepts-ctrl-d-hides]] — partly supersedes
   its catalog-fetched-once claim: a direction flip is an edit that
   re-fetches the catalog; navigation still never does.
