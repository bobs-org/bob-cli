---
keyword: Untracked Sessions Omit T Field
aliases:
  - untracked sessions omit t-field
  - untracked omit t
summary:
  "A timed ledger row without [t::] is an untracked session; writers decide
  trackedness when they write a span and readers only look at the row."
metadata:
  status: accepted
  decided: 2026-11-01
---

# Untracked Sessions Omit T Field

**Applies to.** bob-cli and the vault.

**Claim.** A timed ledger row with `[t:: Nm]` (or the legacy stopwatch) is a
tracked session; a timed row without either is an **untracked session** — on
the ledger, off the clock. Writers decide trackedness when they write a
span; readers only look at the row. `bob gcal sync` splits past MEET
clusters by trackedness (two closed MEETs may overlap: one with `[t::]`,
one without), and a created running MEET is untracked only when every
current event is untracked. MEET starts follow the event run for both span
and `[t::]`. Stats, homes, `bob pomodoro --worked`, and the agenda skip
untracked sessions' minutes and counts.

**Why.** Bryan's mental model is which Pomodoros should receive a `t::`
property or not: a lunch on the calendar should not count as time. Absence
of `[t::]` reads honestly in Obsidian, changes no ledger line grammar, and
is safe — a vault scan found 0 of 1,226 closed timed rows without `[t::]`,
so retiring the span-estimate fallback rewrites no history. Deriving
trackedness at read time from links would be opaque and need link
resolution in every reader.

Rejected alternatives:

- **`[t:: 0m]`.** Reads like a bug and still counts as one Pomodoro.
- **`#untracked` on the Pomodoro line.** Changes the ledger grammar in
  Rust, JS, and Swift parsers.
- **Deriving trackedness from links at read time.** Opaque, needs link
  resolution everywhere, and cannot be overridden per session.

**Cost.** Untracked sessions vanish from every aggregate (minutes, counts,
homes, Day strip). A tag flip after the session closed is reported once and
left for a hand edit; sync never retimes existing MEET entries. Mixed
current events stay tracked (one running entry is allowed).

**Reopens when.** An untracked session needs to count as a Pomodoro, or
`[t:: 0m]` should mean something other than a bug.

Evidence: `plan:202611/untracked_calendar_events.md`; `docs/gcal.md`
§ Untracked events, § Sync; `tests/gcal_sync.rs`; `tests/gcal_lifecycle.rs`.

[[decisions:synced-meetings-link-by-phase]]

[[glossary:event-task]]
