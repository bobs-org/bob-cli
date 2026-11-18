---
keyword: Review Attention Follows The ]s Walk
aliases:
  - s attention follows the walk
  - s predicate
summary:
  "The `s` mark is on exactly when `]s` has a row to land on, wearing the hue
  of the group `]s` enters first; PRE and POST share cyan as bookends and the
  Mac `s` count reads the walk total."
metadata:
  status: accepted
  decided: 2026-11-18
---
*Accepted · project*

**Applies to.** bob-cli `bob walks`; the bob-ledger-tools walker footer and the
bob-navigation-hotkeys selector dot and selected-`s` ink; Bob Mac Capture's
idle-agenda `s` mark.

**Claim.** The `s` mark is on exactly when `]s` has a row to land on — that is,
`attention = available && walk > 0`, with `walk` summed over all nine review
tiers. `tier` is the first non-empty group in walk order (PRE → NEW → PROJECTS →
PENDING → NEXT → RECURRING → TICKLER → ROTTEN → POST): exactly where a fresh
`]s` enters the walk. The label is `Review: {walk} due · {commitments}
commitments — {TIER} first`, with `1 commitment` for one commitment and
`commitments done` for zero. PRE and POST are bookends in the same cyan
(`--color-cyan`, Mac `#00bfbc` / `#53dfdd` PRE pair, ANSI `36;1`);
ROTTEN wears the footer orange (`--color-orange`, Mac `WorkAdviceTone.orange`,
ANSI `38;5;208;1`); every other tier keeps its hue. The Mac `s` count reads
`counts.walk` and falls back to `commitments` only when `walk` is absent, so the
number means "]s landings left" and never reads `0` on a lit mark. See
[[decisions:recurring-occurrences-walk-when-due]] for the tier order and
[[decisions:idle-capture-mirrors-walk-attention]] for the refresh lane the idle
marks ride.

**Why.** Bryan's 2026-11-18 report: the walker footer read `POST 1/2` with two
landable rows, yet the selector's `s` stayed dark and Mac Capture showed no `s`
mark. Both sides defined `s` attention as `commitments > 0`, leaving ROTTEN and
POST out as "upkeep", while `]s` lands on any row in the shared freshness queue.
The predicate now admits every landable row, and POST finally has a hue to wear.
Rejected alternatives:

- **Commitments-only attention.** Keeps the reported dark-`s`-with-landings
  contradiction for ROTTEN- or POST-only states.
- **A quieter upkeep-only mark.** Two marks for one predicate doubles the
  contract every surface must implement.
- **POST dusk hue (`post_hue = dusk`).** A ninth hue blurs at dot size next to
  TICKLER blue and PROJECTS purple; PRE/POST are the same `#gtd` checklist row
  kind and already share teal marks in notes, and footer text (`PRE`/`POST`),
  tooltips, and in-note glyphs carry the difference.
- **Mac `s` count stays commitments (`s_count = commitments`).** A ROTTEN- or
  POST-only mark would show bare or read `0` while lit, disagreeing with the
  label's leading `{walk} due` and the footer's `Review r/N`.

Evidence: `plan:202611/review_attention_follows_walk.md`; bob-cli `aea85529`
(`bob walks`, `docs/walks.md` S1–S7); bob-plugins walk-attention work closed as
bead `bob-cli-n8.2` (`walkerFooterReviewAttentionLabel`, `npm run build`,
`validate`, `docs:check`, synced to the vault); bob-mac-capture `dc39311`
closed as bead `bob-cli-n8.3`, exact-SHA macOS CI run `44337649084` green;
`docs/walks.md` conformance vectors S1–S7.

**Cost.** `s` stays lit through ROTTEN upkeep until the walk reaches 0; a
morning that ends in upkeep keeps its dot to the last landing.

**Reopens when.** `]s` starts skipping a tier, or ROTTEN gains a stopping rule
that removes rows from the walk.
