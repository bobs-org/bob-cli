---
keyword: Decay Decisions Are Available Immediately
aliases:
  - ungated decay
  - no decay trial date
  - immediate decay cards
summary:
  "Gesture-triggered approved-decay cards are available as soon as compatible
  plugins are installed; there is no calendar gate, replacement date, or
  counting-only period."
metadata:
  status: accepted
  decided: 2026-10-04
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** Approved-decay decisions fire on an explicit Alt+F/Alt+Shift+F
gesture as soon as decay is enabled and compatible plugins are loaded
(bob-ledger-tools freshness namespace v6 with `freshnessDecayCard`
version ≥ 2). There is no October 19 switch, replacement date, opt-in,
or calendar-based counting-only period. On an exact, due Ready
ROTTEN/RETURNED task at the keep limit, a single press opens the existing
consent card and writes nothing; counted and Task Link sessions skip
those targets. A card still requires an explicit gesture, never a timer
write. `freshness.decay: false` remains the off-switch. Counting,
eligibility, explicit choices, priority/log writers, and exclusions stay
as [[decisions/rotten-keeps-use-priority-decay]] states them.

Rejected alternatives:

- **Keep or slide the 2026-10-19 gate.** A moved constant still encodes
  obsolete policy and lets CLI, marks, and handlers disagree.
- **A replacement date, opt-in, or counting-only period.** Availability
  is immediate on a compatible install.
- **Timer-fired cards.** The card still needs a gesture.

Evidence:
`plan:202610/freshness_decay_without_trial_date_1.md`;
bob-cli `fc438bc` (`docs/freshness.md` §2a Availability, JSON schema 8
`decay {enabled, keeps, enter}`,
`tests/cli/freshness.rs::list_decides_on_early_dates`);
bob-plugins `f4b3562` (bob-ledger-tools 1.28.0, bob-navigation-hotkeys
2.2.0).

**Cost.** Mixed-version sessions stay truthful: new navigation with an
older ledger falls back to counted keeps; new ledger with older or
missing navigation shows pips without a card promise. Both languages
stay in sync under schema 8 / namespace v6.

**Reopens when.** Mixed-version fallbacks hide a needed card from a
capable install, or a future experiment needs a calendar gate.

Supersedes in part [[decisions/rotten-keeps-use-priority-decay]] for the
activation boundary and trial-extension reopening condition only.
