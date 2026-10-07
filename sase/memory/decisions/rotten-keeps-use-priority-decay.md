---
keyword: Rotten Keeps Decay Through The Priority Ladder
aliases:
  - keep streak decay
  - approved decay
summary:
  "Repeated due-Ready keeps earn an explicit approved decision that enters the
  existing priority ladder; nothing decays silently and freshness itself still
  never changes priority or schedule."
metadata:
  status: superseded-in-part
  decided: 2026-10-03
  superseded_by: decisions/decay-decisions-are-available-immediately
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** The stored `[keeps:: N]` field is a streak of due-Ready bare keeps
(how many consecutive due reviews confirmed the task as written), not a
lifetime refresh counter; absence means 0 and writers omit zero. Only an
exact, due, Ready rotten/tickler target increments, once per review, through
the single JavaScript helper `api.freshness.keepLine`; every other human stamp
clears the streak, and automation, hooks, randomize, and seed neither
increment nor reset. At the configured limit (default 3) the next due
Alt+F/Alt+Shift+F press opens an explicit consent card — Not now, Less often,
Reword, Drop, Keep — and writes nothing until Bryan chooses; Enter can defer
but never cancels from that card. Approved schedule-changing decisions reuse
the existing priority roll/decay ladder and log writers with `· kept N×`
reason tails; Keep anyway just counts again. Freshness itself still never
changes a lane, Today, schedule, or priority. Repeated keeps earn a question
about urgency, never an automatic change.

Rejected alternatives:

- **Lifetime `refresh_count` or a second alias counter.** One streak field;
  no backfill: seeded, captured, and old `fresh` stamps are not evidence
  of keeps.
- **Silent auto-decay at the limit.** The card asks; nothing changes
  without an explicit choice.
- **Counting non-Ready lanes or NEW tasks.** Only due Ready
  rotten/tickler targets count; other targets stamp and preserve.
- **Timer-fired cards.** A card still requires an explicit gesture after
  the 2026-10-19 activation date, never a background write.
- **A new REVIEW LOG structure.** Decisions reuse the existing Schedule,
  Cancel, and Work logs; Keep anyway leaves only the line increment.

Evidence:
`research:202610/rotten_keep_streak_and_approved_decay/rotten_keep_streak_and_approved_decay.md`;
`plan:202610/rotten_keep_streak.md`;
`docs/freshness.md` §§2a/7/10a/11 and `docs/projects.md` ("Approved-decay
decision planner") in bob-cli; `api.freshness.keepLine` (namespace v5) plus
the FreshnessDecayCardModal in bob-ledger-tools 1.24.0 /
bob-navigation-hotkeys 1.69.0; shared vectors
`tests/fixtures/freshness_keeps/vectors.json`.

**Cost.** Two implementations (Rust read/clear/report plus JavaScript
count/display/decide) stay in sync under the shared vectors; the scalar is
not an exact global event counter (git sync may lose an increment, caches
under-count, hand edits stay unobserved). Calibration is a lightweight
human tally, never telemetry or automatic tuning.

**Reopens when.** The post-activation calibration tally shows the limit
misfires (mostly Keep suggests 4, mostly Drop suggests 2), or the recorded
trial extends the 2026-10-19 boundary.

Superseded in part: 2026-10-19 activation boundary and the
trial-extension reopening condition only — see
[[decisions/decay-decisions-are-available-immediately]]. Counting,
explicit consent, priority/log writers, and exclusion rules stand.

Amended in place 2026-10-07 at Bryan's request: the RETURNED walk tier is renamed TICKLER (machine tier `tickler`, footer label TICKS); the Ready state stays `resurfaced`.
