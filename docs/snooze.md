# Snoozed tasks: later-today Task Links

A Snoozed Task is an open, committed, non-Event task planned for **today,
but not before a clock time**. Its Task Link waits as one row in the daily
note's `## Snoozed` section after today's ledger — `- **1530** [[note#^id]]`,
the link's notes beneath — instead of under a Pomodoro. While it waits the
task is **not Today**: no 🍅, absent from the `=`/`x` lineups, the `]s` and
`]w` walks, and the dash PENDING/NEXT sections, and not counted in
`plan.max_links`. At or after its time, `bob snooze wake` moves the link and
its notes to the top of an untimed open Pomodoro of the task's type, never
the running one, and strikes the row with the actual time. Same-day only:
an unwoken row becomes history at midnight and the sticky Next lane returns
the task to review; multi-day deferral is `[scheduled::]`.

This page is the contract. The Rust side is `src/native/snooze/`
(`parse.rs`, `when.rs`, `state.rs`, `plan.rs`, `vectors.rs`); the
JavaScript mirror lives in bob-ledger-tools and runs the shared fixture
verbatim. The machine-readable parity vectors live in
`tests/fixtures/snooze/vectors.json`, which both languages cite: Rust
runs all six groups (plus unit tests); plugins copy the fixture and run
`rows`, `when`, `live`, and `cleanup` verbatim.

This is not the Reading Board's Snoozed lane (a review parking lot with
no clock) and not a project-frontmatter `snooze:` hold (a project-level
deferral). Those keep their names and behavior; this contract owns only
the daily-note `## Snoozed` section described here.

## Row grammar

The section is the first `## Snoozed` H2 in the note. It ends at the next
`## ` heading or at end of file. It is fence-aware and frontmatter-aware,
like the Pomodoros section range. A canonical writer form is a bare
`## Snoozed`; trailing text after the name still opens the section, for
consistency with the Pomodoros heading rule. Later duplicate sections are
linted (`duplicate_headers`) and never acted on.

An active row is `- **HHMM** <block link>`, at column 0, with exactly one
plain or embedded block link and nothing else:

- On read, `HH:MM` and `H:MM` inside the bold are accepted. Writers
  normalize to `**HHMM**`.
- Embedded (`![[…]]`), local (`[[#^id]]`), basename, and aliased links are
  all accepted. Identity compares the resolved vault-relative path plus
  block id, never spelling: basename and block ambiguity resolve through
  the vault link resolver, and two spellings of the same note and block
  id are the same task.
- A struck row is `- ~~**HHMM** <block link>~~ · woke HH:MM`: the day's
  provenance, kept until the heading is deleted.
- Child bullets are the link's authored notes and travel as a subtree.
  They are indented by construction, so they never parse as independent
  rows.
- Writers keep rows sorted by target time, with ties in link order.
- Malformed rows, extra links, extra row text, rows inside fences, and
  indented row-like text are reported (`malformed`, `indented`) and never
  executed. Child notes that merely look row-like stay notes.

A Snoozed row never creates a task mention: it is note prose, vector MN5
in `docs/task-relations.md`.

## Time grammar

The time expression accepts `1530`, `15:30`, `H:MM`, `3:30p`, `3p`, `19`,
`+45m`, `+2h`, and `+1h30m`. A bare `330` means the next 3:30 today:
03:30 when future, else 15:30 when still today, else a past-time refusal.
Relative times round upward to a five-minute boundary. Resolution is in
Bob's zone against an injected clock (`SnoozeClock { date, now_minutes,
zone }`), and this module never reads process state itself:

- strictly future on the same day; past times refuse with teaching copy
  (`same-day only — Schedule (s:) defers to another day`);
- relative targets past midnight refuse as `next_day`;
- DST gaps refuse as `dst_gap`;
- folded wall times resolve to the first occurrence — never silently the
  second when the first has passed (that case refuses as `past`).

`WhenRefusal::message` carries the teaching copy every entry surface
shares. Times render as four-digit `HHMM` and RFC 3339 instants in Bob's
zone; vectors pin input, instant, zone, and result.

## Rows, time, and identity

Snapshot loading and resolution stay separate from pure parsing and
classification: callers resolve each row to exactly one `#task` identity
and supply the task fact plus the live-ledger-link flag, and
`classify_rows` derives the state with no disk writes. Keep `Today`
strictly ledger-derived; snooze state never writes it.

A row is live while all of these hold: it parses, it is unstruck, it is
in today's note, its link resolves to exactly one open `#task` that is
not an Event Task, that task is committed (Next `[*]` or In Progress
`[/]`), and the task has no live link under an open Pomodoro. A live row
whose target is blocked (`[?]`) stays parked. Every other row is
ignored with a reason: `done`, `cancelled`, `already_linked`, `missing`,
`ambiguous`, `ready`, `event_task`, `duplicate`, `malformed`, or
`not_task` (plus the row-model-only `previous_day`, which never appears
in a wake report because wake reads today's note only).

- Per `uncommitted_rows = ignore`, a Ready row is ignored: release stays
  final, and hand-written rows on uncommitted tasks never revive them.
- A live ledger link wins over snooze (`already_linked`).
- An Event row is ignored and also emits the `snoozed_event_task` lint.
- The earliest target time for a resolved task wins; ties use source
  order. The rest are `duplicate`.
- `next_wake_at` (RFC 3339 or null) is the earliest live target.
  Blocked and ignored rows never feed this timer: it is not a busy retry
  for parked rows.

`live_targets` is the documented reader API for the outer planned-today
phase: `planned` holds live-or-blocked identities (the exclusion set for
`]s` scope, recurring and decide rules, the seed, `]w`, walks counts,
and dash queries), and `blocked` distinguishes the parked subset. Ready,
history, duplicate, and ignored rows never enter planned-today. Readers
that mean workable now keep Today.

## Mutations

Pure planners in `snooze/plan.rs` take explicit note snapshots and
return postimages plus metadata. They never shell out or write files.
Callers (the guarded CLI, and later capture's in-memory batch) supply
the daily note, task facts, the identity map, and an injected
`SnoozeClock`. Same-path task and daily-note edits coalesce, including
shifted line offsets after a Ready→Next stamp.

- `plan_add` refuses Event, Blocked, done, cancelled, and non-task
  targets (`event_task`, `blocked`, `done`, `cancelled`, `not_task`) and
  a missing Pomodoros section (`no_pomodoros`). Ready becomes Next via
  `plan_task_link` (and `[fresh::]` when that write changes the line);
  Next and In Progress stay byte-identical. The first live dedicated
  ledger link's notes become the snooze subtree; later dedicated
  duplicates keep those notes as a display bullet; only untimed entries
  this operation emptied are removed; running and closed recorded-time
  entries stay. An existing active row is retimed; a matching history
  row is replaced. Rows sort by target time, then link text. The
  heading is created after Pomodoros when missing.
- `plan_drop` removes every unstruck active row for one identity and
  deletes the heading only when no rows and no prose remain. Task
  fields never change. Absence refuses `not_snoozed`.
- `plan_wake` wakes due live rows in time then source order. An
  explicit identity list bypasses due-time for asleep/due rows and
  never overrides ignored or blocked. Destinations use opt-in
  `never_running` placement: untimed exact, then untimed containing,
  else a typed next-up placeholder after the running entry. No type
  uses the first untimed open entry, else `- [ ] ()`. Each batch of
  the same destination inserts at the top (`FirstChild`, then after
  the previous subtree). Notes travel under the new ledger link; the
  snooze row is struck (`- ~~**HHMM** link~~ · woke HH:MM`) and its
  children are stripped. Wake never edits a task line, never refuses
  on `plan.max_links` (over-cap is reported), and prunes
  done/cancelled/already_linked rows in the same pass. Blocked rows
  stay. An immediate second wake is empty.
- `plan_cleanup` is the capture/plugin reuse of that prune: drop
  done, cancelled, and already-linked active rows; leave blocked,
  Ready, Event, malformed, and history.

Ordinary link routing, MEET placement, and subtree-appending callers
keep their defaults: `place(..., never_running: false)` and
`SubtreeInsert::Append`. The guarded CLI, receipts, tick step, and
writer audit land in later phases.

## Conformance vectors

`tests/fixtures/snooze/vectors.json` owns six groups with documented,
language-neutral schemas (`schemas` in the fixture file):

- `rows`: parse and states — section boundaries, CRLF, fences and
  frontmatter, duplicate sections, embedded/local/aliased links, history,
  malformed and indented rows, children subtrees.
- `when`: the time grammar against an injected now — every accepted
  shape, bare-digit next-occurrence, relative rounding, past and
  midnight refusals, DST gap and fold, invalid shapes.
- `live`: row classification — all task states, already-linked targets,
  duplicates (earliest wins, source-order ties, spelling variants),
  previous-day notes, history exclusion, blocked parking, event lints,
  and `next_wake_at` nullability.
- `snooze`, `wake`, `cleanup`: add/drop, wake, and cleanup plans.
  The typed loader (`vectors.rs`) rejects unknown groups, runners, and
  modes instead of skipping them.

Plugins copy the fixture byte-for-byte into their
`scripts/fixtures/snooze/` and run the `rows`, `when`, `live`, and
`cleanup` groups verbatim.
