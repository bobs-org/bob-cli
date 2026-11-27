---
keyword: Cabinets Absorb Catalogs At Render Time
aliases:
  - cabinets absorb catalogs
  - absorb catalogs at render time
summary:
  "Property-card cabinets render bob-owned catalogs at render time: the
  stored callouts stay byte-for-byte bob's, and nothing hides without a
  working cabinet."
metadata:
  status: accepted
  decided: 2026-11-27
---

**Applies to.** bob-plugins and the Bob vault.

**Claim.** Cabinets render bob-owned catalogs at render time, the stored
callouts stay byte-for-byte bob's, and nothing hides without a working
cabinet.

**Why.** The research report's unanimous points 3–4 and the readers of the
bytes: vault readers keep seeing the stored catalogs, so cabinets must never
move, rewrite, or remove them.

Rejected alternatives:

- **Literal H2 removal.** Destroys the stored catalogs the readers need.
- **Report-JSON bodies.** Renders cached data instead of the note's bytes,
  so staleness hides catalogs.
- **Nested scrollers.** Traps scroll inside the card and breaks walk and
  reveal.
- **A per-note H2 order.** Lets each note order its own sections, losing
  the one fixed stack every card shares.

**Cost.** The Obsidian Outline still lists the stored headings.

**Reopens when.** Bryan wants catalogs out of the Markdown, or mobile gains
cards.

**Evidence.** `plan:202611/property_card_cabinets.md`; bob-plugins
`64268af9` (cabinets go live, gate deleted) and `680ee414` (cabinets
reference and family pages).

Links [[glossary:cabinet]] (the compartment this rule governs).
