---
keyword: Note Ready Cap Counts The Lane
aliases:
  - per-note ready cap
  - crowded notes
  - ready cap
summary:
  "The per-note soft cap (plan.max_ready_per_note, default 5; ready_cap: N|off)
  counts each area/project note's whole Ready lane by residence, whatever its
  freshness."
metadata:
  status: accepted
  decided: 2026-10-01
---

**Applies to.** bob-cli, bob-plugins, vault, dash.

**Claim.** Every area/project note has a soft cap on its Ready lane
(`plan.max_ready_per_note`, default 5; per-note `ready_cap: N|off`). The
cap counts each note's whole Ready lane by residence, whatever its
freshness. Recurring and `^prj` rows are excluded. Crowded means strictly
over; full is neutral. Nothing is refused, written, or stored. The dash
chip order becomes NEW, PENDING, NEXT, READY, CROWDED, BLOCKED, ROTTEN,
TODAY.

Rejected alternatives:

- **Gated per-note count.** Rot cliff and review inversion: skipping
  review would lower the count without lowering pressure.
- **OPEN/SHOWN vocabulary.** Crowded, full, and room stay the words.
- **Hard refusal of moves.** Crowded notes never fail the report.
- **Auto-defer or auto-split.** The fix stays split, sequence, defer,
  or drop by hand.
- **Stored counts.** Counts stay a read-time evaluation; frontmatter
  holds only the `ready_cap` override.
- **Hooks badge row.** Lane semantics stay unchanged.
- **Parent rollups.** Residence only; never parent, heading, embed,
  or backlink.
- **Separate area default.** One global key plus the per-note
  override.
- **Amber at-limit color.** Full is neutral; only crowded is red.
- **Folding into the READY chip.** CROWDED is its own chip.

Evidence:
`research:202610/per_note_ready_cap/per_note_ready_cap.md`;
`plan:202610/per_note_ready_cap.md`;
`docs/plan.md` Ready cap per note in bob-cli.

**Cost.** Per-note counts don't sum to READY; two implementations
(Rust plus JavaScript) stay in sync under vectors R1–R14; the mobile
default cap applies where `config.yml` is unreadable.

**Reopens when.** Bryan finds the cap unhelpful after tuning N and the
notice scope.

Amended in place 2026-10-04 at Bryan's request: the freshness trial was removed; nothing waits on it.
