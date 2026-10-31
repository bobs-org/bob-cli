---
keyword: Gcal Root Follows Google Until Edited
aliases:
  - unedited gcal root follows Google
  - gcal root reroute
summary:
  "An Event Task's [root::] follows Google's chosen root until Bryan edits it;
  his edits always win and are reported once on conflict."
metadata:
  status: accepted
  decided: 2026-10-31
---

# Gcal Root Follows Google Until Edited

**Applies to.** bob-cli and the vault.

**Claim.** An Event Task's `[root::]` follows Google's chosen root — a
title or description `@route` token, colour, calendar, or default; never
series memory — while the vault value still matches what Bob last wrote.
Bryan's `[root::]` edits always win. When Google later routes elsewhere,
sync reports that conflict once (`kept_root`) and leaves the edit. A
reroute adds `[type:: MEET]` only when the line has no type field. Span
updates (time, title, badges) stay independent of the root decision: a
kept title does not block a reroute, and a kept root does not block a
time change.

**Why.** Write-once roots left the Halloween 2026-10-31 row at
`[root:: meet]` after Bryan added `@love` to the Google event: `choose_event_root`
stripped the token, `apply_span_updates` kept the journaled root, and
`bob gcal list` showed `● love` against a vault `meet` with no warning.
Rerouting from series memory would flip-flop: editing one occurrence back
to its original value looks unedited (`current == recorded`) and would be
rewritten to the stale learned root, then flip again once the series
re-learns. Folding root into the span three-way merge would stop time
updates from reaching every manually rerouted meeting.

Rejected alternatives:

- **Write-once roots.** Silent Halloween failure; list and the vault
  disagree with no notice.
- **Reroute from series memory.** Flip-flops on a change-back.
- **Fold root into the span merge.** Blocks time updates on rows Bryan
  already rerouted.

Evidence: `plan:202610/gcal_reroute_unedited_root.md`; `.bob/gcal/athena.jsonl`
Halloween row `"root":{"source":"default","value":"meet"}`; `docs/gcal.md`
§ Who owns each part of a row, § Reconciling Google-side changes, § Root
selection; `tests/gcal_lifecycle.rs`.

**Cost.** Google-side colour, calendar, default, or token changes rewrite
unedited `[root::]` values, including done and already-worked rows, and may
add `[type:: MEET]`. Bryan's edits still win and are reported once.

**Reopens when.** `bob gcal list` needs to show a vault-edited root beside
the chosen root, or HTML-formatted description tokens should participate.

[[decisions:bob-gcal-is-read-only-and-proposes-once]]

[[glossary:event-task]]
