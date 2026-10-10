---
keyword: An Empty Capture Draft Shows Today's Ledger Agenda
aliases:
  - idle agenda
  - ledger agenda
  - agenda caching folding eye line
summary:
  "An empty capture draft shows bob's `capture-pomodoros --tasks` agenda (Now,
  Next, Later), cached in app memory and revalidated on filtered vault events and
  show; folds go farthest-first (logs, one-line, one-row, name strip) below a fixed
  eye line; the app never reads the vault."
metadata:
  status: accepted
  decided: 2026-10-09
---

**Applies to.** Bob Mac Capture (agenda cache, fit planner, panel layout) and bob-cli
(`bob capture-pomodoros --tasks` as the agenda's only fact source).

**Claim.** An empty capture draft shows today's ledger agenda, painted from memory in
the first frame:

1. **bob owns every fact.** Roles, Task Link recognition, `=x`/`=` numbering, link
   resolution, task blocks, line kinds and depths, log tagging, clean titles,
   statuses, the date, and the completed summary all come from
   `bob capture-pomodoros --tasks`. See [[mac-capture-is-a-thin-client]] and
   [[today-is-read-from-the-ledger]].
2. **The app owns caching, fitting, and pixels — and never reads the vault.** The last
   good snapshot lives in app memory and is revalidated on launch, on filtered vault
   events (visible `.md` notes only), on every panel show (stale-while-revalidate),
   after each submit, and on wake, unlock, and day change. Byte-identical output
   publishes nothing. A snapshot whose date is not today is never shown; a failed
   refresh keeps the last good agenda and marks it stale.
3. **Folds go farthest-first below a fixed eye line.** When the agenda does not fit,
   the farthest Pomodoros lose detail first: logs, then one-line tasks, then one row
   per Pomodoro, then the Later name strip. Next and Now fold last. Every fold leaves
   a chip naming what is hidden, and expanding a chip never moves the editor: the
   agenda is capped at the below-eye-line budget and never scrolls.
4. **Numbers match the session operators.** The current entry shows its `=x` numbers;
   every other open entry shows its `=` lineup number, because `=#NAME~K` drops by
   those same numbers. The Now countdown ticks at minute granularity while visible.

**Why.** The question Bryan most often has when opening the panel — "what am I doing,
and what's next?" — is already answered by today's ledger, so the empty draft shows
it instead of dead space, with no new concept. Caching in memory (not on disk, not in
a daemon) keeps the first frame instant on a resident app that prefetches at launch.
Farthest-first folding keeps Now fully detailed on heavy days, when recency is what
matters. The fixed eye line keeps the caret still however the agenda folds, types, or
refreshes. Rejected alternatives:

- **A daily-file cache key.** Stale-while-revalidate on filtered events is fresher
  than keying on the day file, which misses task-note edits entirely.
- **A global fold ladder.** Per-Pomodoro units with distance-ordered folding preserve
  Now; a global ladder would thin the running Pomodoro first on heavy days.
- **A horizon cap.** Dropping far Pomodoros hides actionable `=#NAME~K` numbers; the
  name strip keeps every entry reachable in one row.
- **Swift-side vault reads.** The app never parses the daily note or task notes and
  never infers current/next from the clock; every ledger judgment stays in bob.
- **Building on `bob plan`.** `plan` answers a different question (backlog order, not
  today's ledger lineup with operator numbers).
- **A fixed-height scroll list.** Scrolling moves content under a fixed caret; the
  fold ladder guarantees the agenda always fits without scrolling.

Evidence:
`research:202610/idle_capture_pomodoro_agenda/idle_capture_pomodoro_agenda.md`;
epic `bob-cli-66` (`plan:202610/idle_capture_pomodoro_agenda.md`); phases
`bob-cli-66.1` (`bob capture-pomodoros --tasks`, `docs/capture.md`, goldens),
`bob-cli-66.2` through `bob-cli-66.6` (models, store, planner, view, polish in
bob-mac-capture, CI green at `f8c9c28`).

**Cost.** One in-memory snapshot plus a background refresh per trigger; chips and
expansions add per-unit view state that resets on hide and snapshot change. The
countdown ticks only while the panel is visible. Stale marking is the only promise
when a refresh fails — yesterday's plan is never shown, but a broken link or a dead
`bob` still needs the warning rows.

**Reopens when.** The agenda needs to write the vault (row clicks, `⌘1…9` insertion),
navigate by keyboard, or show completed Pomodoros — any of which changes what the
cache must hold and what a row may do.
