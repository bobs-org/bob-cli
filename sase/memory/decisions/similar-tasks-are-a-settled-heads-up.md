---
keyword: Similar Tasks Are A Settled Heads-Up
summary: Capture shows a "Similar tasks" band only after typing settles and only
  when a match clears bob's calibrated bars; every row action is an explicit draft
  edit and Return never changes.
metadata:
  status: accepted
  decided: 2026-10-21
title: Similar Tasks Are A Settled Heads-Up
aliases:
- similar-tasks-are-a-settled-heads-up
---

# Similar Tasks Are A Settled Heads-Up

*Accepted · project*

aka similar tasks band, duplicate heads-up

**Applies to.** Bob Mac Capture, bob-cli.

**Claim.** When a capture draft will create new tasks, the preview shows a
"Similar tasks" band at the foot of the new task's own preview card. It is a
heads-up, never a gate:

- A row carries a **Same title** or **Possible duplicate** mark only when the
  evidence supports it. No percentages, and no score is ever shown as a
  probability.
- **Zero pixels unless a match clears the bar.** No header, no placeholder,
  never a "No duplicates" line.
- **Settle-gated.** bob is asked after about 350 ms without edits, on its own
  cancellable lane. Rows on screen hold while the user types and swap only if
  the settled result changed.
- **Scope.** Every open task in routable notes, plus tasks closed in the last
  30 days at the duplicate bar only (at most 1 row).
- **Gate.** Only items that create a task: `task`, `pomodoro_task`, and the
  `^prj` title of `project_note` / `pomodoro_project_note`. Require at least 2
  meaningful terms, with a stricter bar under 3 terms except for an exact title
  match. `@@template:` drafts are skipped.
- **Cap.** 2 rows for a single-item draft plus a "+N" capsule, 1 row per item
  in bulk drafts, at most 5 per draft.
- **Actions are explicit draft edits only.** The strip's keys are the capture
  sigils (`~` relate, `+` add as note, `&` needs it, `*` blocks it, `/` open,
  `⌫` discard mine). Return is never intercepted or re-confirmed, nothing is
  auto-linked, and `@` and `!` are not v1 actions. The ↩ default is split:
  ⌫ Discard mine for Same title, + Add as note for Possible duplicate.
- **Numbers.** Zero extra spawns per keystroke. Preview p95 is unchanged with
  the feature on versus off. The related lane's p95 on the Mac is at most
  50 ms.
- **Settings toggle** ("Show similar tasks while capturing", on by default).
  Thresholds live in bob, not in Settings.
- **Calibrated bars** (`bob capture-related`, schema_version 1):
  `POSSIBLE_DUPLICATE` 0.70, `SIMILAR` 0.48, `CLOSED_DUPLICATE` 0.70,
  `HOLD_MARGIN` 0.10, `SHORT_QUERY_BOOST` 0.10, `MIN_TERMS` 2 — with
  closed-class function-word stopwords, single-digit numbers kept as terms,
  and a number guard (titles whose number sets differ are never duplicates).
  Measured on the 496-query leave-one-out census: possible_duplicate precision
  6/6, similar usefulness 39/54 (72.2%), firing 55/496 (11.1%, about 1 capture
  in 9). Mac release p95 is 43.494 ms against the 50 ms gate.
- **bob owns meaning; the app owns timing and pixels.** Eligibility, tokens,
  weights, bands, hysteresis, exclusions, action availability, defaults,
  splice anchors, and replacement strings come from `bob capture-related`.
  The app schedules the call, holds and fits rows, and applies the edits bob
  describes.

**Why.** The research showed wording reliably finds the same work captured
twice but does not find workflow relations: on 95 real dependency pairs the
true prerequisite ranked top-5 only about 20% of the time — so the UI promises
a duplicate heads-up, not relations. The Mac turned out fast (4.7 ms spawn,
21 ms open-task scan), so one per-settle subprocess call meets the 50 ms gate
with no in-app index. TF-IDF over Bob-aware tokens beat the picker's
all-terms matcher (3% fire rate, missed paraphrases) and plain Dice (missed
stemmed duplicates). Calibrated from 80 reviewed leave-one-out pairs labeled
under Bryan's delegation (10 D / 47 U / 23 N): scalar thresholds alone capped
possible_duplicate precision at 60% until the number guard and stopword fixes
reached 6/6. These are agent judgments, not Bryan-authored labels, and the
figures are in-sample, not held-out.

Rejected alternatives:

- **A "related tasks" list.** Matching cannot deliver workflow relations, so
  the band must not promise them.
- **An in-app index for v1.** Measured latency leaves headroom; the index
  stays a measured escalation path if the gate ever breaks.
- **Embeddings, Dice, or the picker's all-terms matcher.** Unjustified cost,
  missed stemmed duplicates, or a 3% fire rate respectively.
- **↩ adds as note for both (`duplicate_default = note_both`).** An exact
  recapture is almost always a forgotten task; a lexical near-match may not
  be.
- **Shipping the research's starting bars (`calibration = starting_bars`).**
  The bars come from labels, and the starting 0.55 similar bar fired at only
  6.7% with 60% duplicate precision.
- **A closed bar above 0.82.** It would read 2/2 but rests on one pair;
  re-check when more closed pairs accumulate.
- **An abbreviation table** (appt vs appointment). Vocabulary-specific; add
  only if future labels show recurring misses.

**Cost.** Every capture feature lands twice, bob-cli first, and the Mac half
is gated by macOS CI. The JSON contract grows additively at schema_version 1
with old-bob fallback. Thresholds retune in bob, never in Settings. The band
folds to one line before it can cause scrolling, and held rows render only to
keep an already-visible row steady.

**Reopens when.** The 50 ms gate breaks on measured Mac hardware (escalate to
the in-app index); closed-pair evidence accumulates enough to revisit the
closed bar; abbreviation misses recur in labels; or firing drifts far from
about 1 capture in 9 on real captures rather than the title-census proxy.

Evidence: `research:202610/capture_similar_tasks_band/capture_similar_tasks_band.md`;
`plan:202610/capture_similar_tasks_band.md`; epic `bob-cli-ab` phases
`bob-cli-ab.1`–`bob-cli-ab.7`; bob-cli `787d62cb` (engine), `062fc3da`
(contract), `453dc4a7` (calibration); calibration report and Mac timing in the
research sidecar (`202610/capture_similar_tasks_band/calibration.md`,
`calibration_mac_timing_final.json`).

[[decisions:mac-capture-is-a-thin-client]]

[[decisions:unblocked-tasks-are-offered]]

[[decisions:capture-link-drops-blocking-deps]]

[[decisions:idle-capture-shows-ledger-agenda]]
