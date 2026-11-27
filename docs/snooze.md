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
(`parse.rs`, `when.rs`, `state.rs`, `plan.rs`, `vectors.rs`, `cli.rs`,
`shell.rs`); the
JavaScript mirror lives in bob-ledger-tools and runs the shared fixture
verbatim. The machine-readable parity vectors live in
`tests/fixtures/snooze/vectors.json`, which both languages cite: Rust
runs all six groups (plus unit tests); plugins copy the fixture and run
`rows`, `when`, `live`, and `cleanup` verbatim.

This is not the Reading Board's Snoozed lane (a review parking lot with
no clock) and not a project-frontmatter `snooze:` hold (a project-level
deferral). Those keep their names and behavior; this contract owns only
the daily-note `## Snoozed` section described here.

Display in Obsidian — the definition clock, woken pill, row paint, heading
summary, and plan chip — is a separate contract: [Snooze
marks](snooze-marks.md). This page owns behavior and JSON; that page owns
pixels and the executable `SM-*` vectors.

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

`parse_snooze_when(input, clock)` is the shared parser; capture's `z:`
token reuses it, and `WhenRefusal::message` carries the teaching copy
every entry surface shares. Times render as four-digit `HHMM` and RFC 3339 instants in Bob's
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
`[/]`), and the task has no live dedicated Task Link under an open
Pomodoro. This is exactly the shared `list_queued_links` rule from
`capture_pomodoro_start.rs`: one direct child bullet at the entry's first
child indentation, whose marker-stripped body is exactly one plain or
embedded block link, not struck or fenced. Prose mentions, nested notes,
fenced links, and struck links do not make a snoozed task already linked.
List, wake, forecast, Today, and reconcile use the same membership rule.
A live row
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
that mean workable now keep Today. The plan forecast
(`docs/plan.md#snoozed-forecast`) reuses this set for its `count` meter
and narrows it to on-clock Asleep/Due rows for its nonbinding wake
forecast.

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
  stay. An immediate second wake is empty. A wake whose never-running
  destination cannot be placed refuses the whole batch with
  `no_pomodoros` or `multiple_open_timed`, exit 2, and writes nothing;
  a wake that wakes nothing never refuses for a malformed ledger.
- `plan_cleanup` is the capture/plugin reuse of that prune: drop
  done, cancelled, and already-linked active rows; leave blocked,
  Ready, Event, malformed, and history.

Ordinary link routing, MEET placement, and subtree-appending callers
keep their defaults: `place(..., never_running: false)` and
`SubtreeInsert::Append`. The guarded CLI below is the first caller;
capture's `z:` batch reuses the same planners.

## CLI

`bob snooze` is the read-only list command. The public commands are:

```text
bob snooze [--bob-dir DIR] [--format human|json] [--quiet]
bob snooze add TASK WHEN [--bob-dir DIR] [--dry-run] [--format human|json] [--quiet]
bob snooze drop TASK [--bob-dir DIR] [--dry-run] [--format human|json] [--quiet]
bob snooze wake [TASK ...] [--bob-dir DIR] [--dry-run] [--format human|json] [--quiet]
```

`TASK` is `note:block-id` or `[[note#^block-id]]`. `WHEN` uses the time
grammar above. `-b`, `-d`, `-f`, and `-q` are the short forms of
`--bob-dir`, `--dry-run`, `--format`, and `--quiet`; `-h` shows help.
The vault defaults to `BOB_DIR` (then `~/bob`), and `BOB_DAY_FILE` can
select the daily note. `BOB_NOW` and Bob's configured timezone set the
clock used by every command. `--quiet` suppresses successful human output;
JSON output remains complete.

List never takes the vault lock. Add, drop, and wake first validate and
plan against a read snapshot. A dry run returns that plan without locking
or writing. A real write waits at most ten seconds for the vault lock,
refreshes its inputs and replans once if a preimage changed, and defers
after a second drift or a day rollover. Add settles each note it rewrites
(a promoted task lands in its status group) and stages the task note and
daily note in one rollback-capable batch. Drop and wake write only the daily
note. An automatic wake sweep runs only on the configured writer host;
`wake TASK` is an explicit request and works on any host. A skipped sweep
does not mutate the vault or create state files. A wake whose never-running
destination cannot be placed refuses the whole batch with `no_pomodoros`
or `multiple_open_timed`, exit 2, and writes nothing; dry runs refuse the
same way. Exit status is 0 for success, no work, deferred, or skipped; 1
for runtime failures; and 2 for usage errors or refusals.

Every JSON report has `schema_version: 1`, `date`, and RFC 3339 `now`.
The list report adds `rows`, `next_wake_at`, `plan`, canonical absolute
`vault_root`, and `wake_notice_version: 1`. The version is the capability
probe for wake-notice ownership and eligibility fields; a client should
publish a notifier claim only after seeing version 1. Each row reports its
source `line`, resolved `route` when available, `block_id`, task `title`,
four-digit `target`, `state`, an ignored `reason` when present, and `woke_at`
for history. `wake_notice_eligible` is true only for today's valid struck
history whose identity uniquely resolves to an ordinary committed open,
unblocked task that still has a dedicated Task Link under an open Pomodoro,
with no replacement active Snoozed row for that identity. It is false for
every other row. This read-only field does not alter `state` or history
display. `next_wake_at` is the earliest live target or null; blocked rows do
not drive it. The plan reports current on-clock links, `cap`, and whether the
ledger is over cap.

Add reports `status` (`added` or `planned`), the task identity and title,
the resolved target time and instant, the resulting lane, `daily_file`,
the link-count plan, and `dry_run`. Drop reports `status` (`dropped` or
`planned`), `daily_file`, and `dry_run`. Wake reports `status` (`woke`,
`nothing_due`, `deferred`, or `skipped`), `woken`, `dropped`, `ignored`,
`pending`, `next_wake_at`, `plan`, `daily_file`, `notified`, and
`wake_id`. After a successful commit with a nonempty `woken[]`, wake
writes `$XDG_STATE_HOME/bob-cli/snooze/last-wake.json` (mode 0600 from
creation, then an atomic rename). Before that write, Bob selects one
notification owner: `auto` selects `mac_capture` when the sibling
`notifier.json` contains a live claim, and otherwise selects `desktop`;
explicit `desktop` always selects `desktop`; `off` selects `off`. The
receipt records the intended `notification_channel` plus canonical
absolute `vault_root`; `daily_file` remains vault-relative. The receipt
and command report share one `wake_id`. Darwin notification
`org.bobs.bob.snooze.woken` is posted only for the `mac_capture` owner,
after the receipt is readable. The file carries `schema_version`,
`wake_id`, `date`, `woken_at`, `host`, `notification_channel`,
`vault_root`, `daily_file`, `tasks[]` (`title`, `route`, `block_id`,
`target`, `woke_at`, `destination`, `line`), and `plan`. The channel
records ownership, not proof that operating-system delivery succeeded.
`notified` reports actual dispatch: `mac_capture` after a successful
Darwin post, `desktop` after the desktop helper sent, and `none` when
dispatch failed, the owner is `off`, or no tasks woke. A failed desktop
send does not reroute to Mac Capture. An unreadable or failed receipt
leaves `wake_id` null, skips every notification, and does not roll back
the vault. Dry runs, deferred writes, skipped sweeps, prune-only
installs, and nothing-due wakes leave the prior receipt unchanged. A
wake batch reports each task's destination, target, actual wake time,
and whether that insert crossed the link cap. No command forges a
notification receipt.

`bob capture-pomodoros --tasks` also publishes this read-only section as
`snoozed: {rows, next_wake_at}` in its schema-1 agenda payload. It includes
only today's Asleep, Due, and Blocked rows, with resolved vault-relative
`route`, `block_id`, clean `title`, four-digit `target`, the target day's
RFC 3339 `wake_at`, `state`, and the authored child `notes`. A DST-gap wall
time in a hand-authored row has `wake_at: null`; guarded snooze writes cannot
create one. `next_wake_at` uses the same earliest-live rule as `bob snooze
list`, so Blocked rows do not arm it. These rows stay separate from ledger
links and do not make a task a Today task.

Struck history from today's Snoozed section is matched by resolved note path
and block ID to resolved open tasks under open Pomodoros. Matching agenda
items add `woke_at` (`HH:MM`) and `snoozed_target` (`HHMM`); the agenda date
provides the day. Malformed, unresolved, duplicate-ID, and non-open task
history is ignored. If hand-authored history repeats an identity, the latest
actual wake wins, with the later source row breaking equal-time ties. The
fields are omitted without a match and are not aged out by the CLI.

For example, `bob snooze list -f json` returns a stable envelope such as:

```json
{
  "schema_version": 1,
  "wake_notice_version": 1,
  "vault_root": "/vault/example",
  "date": "2026-10-08",
  "now": "2026-10-08T12:00:00-04:00",
  "rows": [],
  "next_wake_at": null,
  "plan": { "count": 3, "cap": 10, "over": false }
}
```

## Obsidian gestures

In Obsidian, `Alt+Z` (`bob-navigation-hotkeys:snooze-task`) opens the
Snooze Card for one task, dedicated Task Link, or Snoozed row; `z` on the
Task Card opens the same card. The card takes one task only and refuses
counted or multi-target use, Event and effectively Blocked targets, and
past or next-day times (`same-day only — Schedule (s:) defers to another
day`). Wake now delegates to `bob snooze wake`; add, retime, and unsnooze
commit through the guarded editor transactions. The daily note's whole
snooze undoes with one `Ctrl+Z`; a same-note task and daily edit composes
into one transaction, while cross-note task and daily edits undo per
editor. Cancellation writes nothing. When `Alt+Z` conflicts with an
existing binding, the command and Task Card entry stay available and the
existing binding is preserved.

## Writer preservation

The section is trailing on purpose: every ledger writer is section-bounded
and leaves the Snoozed bytes untouched. `bob capture` link, start, close,
`x=` record, and `=` lineup writes end at the next H2, so Snoozed
subtree bytes stay identical, with LF and CRLF coverage where helpers
manipulate offsets. Google Events insertion and MEET filing place rows in
`## Events` and links in `## Pomodoros` without touching the section.
Reconcile keeps Snoozed rows and children byte-identical and confines link
cleanup to `## Pomodoros`. `bob task archive` link repair is the one
exception: when a task's note moves, it rewrites only the intended target
path inside rows and children, leaving time, strike, notes, and every
other byte intact. `Snoozed` is a structural H2, never a user drawer, and
rows are note prose for task mentions (MN5), so no writer treats the
section as task context.

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
`cleanup` groups verbatim. Display conformance is a second fixture,
`tests/fixtures/snooze/marks.json`, owned by [Snooze
marks](snooze-marks.md); plugins copy it the same way and run its
`definition`, `woken`, `rows`, and `plan` groups verbatim.
