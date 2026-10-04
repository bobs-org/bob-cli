# Task freshness

Confirmed tasks carry `[fresh:: YYYY-MM-DD]`: the local calendar date a human
last confirmed that the task still needs doing as written — its wording,
priority, project, schedule, and dependencies. A visible, non-recurring Ready
task that was never confirmed, whose scheduled deferral returned after its
last confirmation, or whose review interval expired is due for review.
Today-linked tasks and canonical daily-note tasks are outside this review
scope. The morning review then costs roughly "pool ÷ interval +
arrivals" glances instead of the whole pool.

The daily lane review reuses the same `[fresh::]` stamp for Pending
(`[/]`) and Next (`[*]`) tasks: each lane has a review cadence
(`freshness.pending_interval` / `freshness.next_interval`, default 1
day), and the morning walk visits NEW → PROJECTS → PENDING → NEXT →
RETURNED → REFERENCES → ROTTEN in explicit tiers.

This file is the contract both implementations cite. The Rust side is
`src/native/freshness/` (`placement.rs`, `state.rs`) with the
`freshness:` config block in `src/native/config/freshness.rs`; the
JavaScript mirror is `api.freshness` in bob-ledger-tools (top-level
api v3, freshness namespace v6). The bob-ledger-tools JavaScript
tests use the conformance vectors below verbatim.

The keep-streak contract (`keeps`, `decay`, introduced in schema 4) is specified
here and implemented in Rust in the contract-rust phase. Its
machine-readable parity vectors live in
`tests/fixtures/freshness_keeps/vectors.json`, which both languages
cite: Rust runs the read/reset/placement cases (it has no production
increment API); the sole increment helper is JavaScript
`api.freshness.keepLine`, which landed with the ledger-marks phase
alongside freshness namespace v5 counting, folded pips, and
count-truthful tooltips. The decision card landed with the
decision-card phase (bob-navigation-hotkeys 1.69.0, bob-ledger-tools
1.24.0) and is available immediately on compatible plugins
(freshness namespace v6, card capability v2): single Alt+F/Alt+Shift+F
presses on exact, due, at-limit tasks open the consent card and write
nothing, counted and Task Link sessions skip those targets without
changing fresh/count, and marks show the leaf with `Alt+F to decide`
only when the card capability is present and decay is on.
Mixed-version sessions keep counting pips with counting-only wording
and no leaf.

## 1. Definition

- **Task freshness** (aka **freshness**) is the local calendar date a
  human last confirmed that an open task still needs doing as written.
  It is stored as `[fresh:: YYYY-MM-DD]`.
- **Stamp / refresh:** write today's date into `fresh`.
- **Due for review:** an in-scope task in state NEW, RESURFACED, or
  ROTTEN (see §4).
- **Refreshed today:** a task whose `fresh` equals today.

A missing stamp — not `created` — means new. `bob gkeep pull` sets
`created` from the Keep note's creation time
(`src/native/gkeep/render.rs`), so `created` records when the thought
was first captured elsewhere, not when Bryan first saw it in the
vault. Every new capture therefore arrives without `fresh` and is due
as NEW until a human confirms it.

## 2. Fields, overrides, and config

| What                           | Where                                          | Value                                    |
| ------------------------------ | ---------------------------------------------- | ---------------------------------------- |
| `[fresh:: YYYY-MM-DD]`         | task line, before the trailing Tasks suffix    | optional; absence means "never confirmed" |
| `[refresh:: N]`                | task line, immediately after `fresh`           | optional integer days, 1–365             |
| `[keeps:: N]`                  | task line, immediately after `refresh`         | optional streak of due-Ready keeps, 1–999; absence means 0 (§2a) |
| `task_refresh: N`              | frontmatter of the note containing the task    | optional integer days, 1–365             |
| `freshness.interval`           | `~/.config/bob/config.yml`                     | integer days, 1–365, default 7           |
| `freshness.pending_interval`   | `~/.config/bob/config.yml`                     | integer days, 1–365, or `false`; default 1 |
| `freshness.next_interval`      | `~/.config/bob/config.yml`                     | integer days, 1–365, or `false`; default 1 |
| `freshness.project_interval`   | `~/.config/bob/config.yml`                     | optional integer days, 1–365; absent/null inherits |
| `freshness.reference_interval` | `~/.config/bob/config.yml`                     | optional integer days, 1–365; absent/null inherits |
| `freshness.rotten_daily_budget` | `~/.config/bob/config.yml`                    | optional integer ≥ 1, default off        |
| `freshness.decay`              | `~/.config/bob/config.yml`                     | keep-streak policy: mapping, `true`, `false`, or null (§2a) |

**Interval precedence.** A configured `project_interval` overrides
every other level for `^prj` rows (source `project`); a configured
`reference_interval` overrides every other level for `^ref` rows
(source `reference`) — task `refresh`, note `task_refresh`, global
`interval`, and the lane interval alike. A tracker without a
configured cadence uses the Ready chain in every lane, never the
lane interval. Otherwise `interval(t)` for an ordinary lane task in
a walked lane is that lane's interval (`pending_interval` for `[/]`
with source `pending`, `next_interval` for `[*]` with source
`next`), overriding the whole Ready chain below. Otherwise
`interval(t)` is the task's `refresh`, then the containing note's
`task_refresh`, then `freshness.interval`, then 7. The note override
applies by residence (the note that contains the task), not through
`parent` links. `pending_interval: false` / `next_interval: false`
turns that lane's walk off for ordinary tasks, but never tracker
review — PROJECTS and REFERENCES walk even with the lane off; an
absent or null lane value means the default 1, matching how
`interval:` already treats null. Absent/null tracker keys inherit
the previous cadence exactly.

**Invalid values.**

- An invalid task or note value is linted and falls through to the
  next level.
- An invalid `freshness:` block is a config error. `bob freshness`
  exits 2; ledger-tools falls back to the defaults and marks them
  `invalid`.
- Unknown keys are ignored, and a mistyped `freshness:` must not break
  any other config loader (it is read as a raw `serde_yaml::Value`,
  mirroring how `plan:` is loaded).

Example:

```yaml
freshness:
  interval: 7 # Ready backlog review cadence
  pending_interval: 1 # [/] lane daily review; false = not walked
  next_interval: 1 # [*] lane daily review; false = not walked
  # project_interval: 1 # ^prj review cadence; absent/null inherits
  # reference_interval: 7 # ^ref review cadence; absent/null inherits
  # rotten_daily_budget: 15 # counts upkeep outside the lanes; never hides tasks
```

Booleans (including `false`), zero, negatives, values above 365,
fractional numbers, strings, and containers are config errors for the
tracker keys; `bob freshness` exits 2 while ledger-tools uses the
complete default config and flags it invalid. Other config loaders
still ignore a malformed freshness block.

**Mobile config caveat.** On mobile, when the config file is
unavailable, ledger-tools uses the defaults above and marks them
`invalid` rather than failing; a config edit on desktop is visible
within the existing 60-second tick. `bob freshness` on desktop still
exits 2 for an invalid `freshness:` block. The mobile/missing-file
fallback carries neither tracker override and cannot see Bryan's
desktop settings.

## 2a. Keep streaks and approved-decay policy

**Storage.** `[keeps:: N]` is a streak of due-Ready bare keeps: how
many times in a row a due Ready task was confirmed without any other
change. It is a decimal integer 1–999; absence means 0 and writers
omit zero. Increment saturates at 999 and never wraps. Readers
accept existing bracket or paren field syntax, select the first
valid value, and emit `keeps_invalid` and `keeps_duplicate` as
appropriate; if none is valid, they report 0. `fresh_misplaced` also
covers `keeps` inside the Tasks suffix. A successful keep
canonicalizes and repairs malformed/duplicate placement. Generic
stamps remove all `keeps` fields. An uncounted keep preserves the
valid semantic value while canonicalizing it. Do not introduce a
second lifetime counter or alias.

Canonical form:

```markdown
- [ ] #task Rename queue input [fresh:: 2026-10-08] [refresh:: 14] [keeps:: 2]
      [created:: 2026-09-12] [priority:: medium] ^rq
```

Output order is `fresh`, optional `refresh`, optional `keeps`, then
the existing Tasks suffix, tags, and block ID. `keeps` extends the
suffix scan as a run-extending **non-Tasks** key; it is never added
to the Tasks key registry. Both Rust parsers see the same Tasks
fields before and after writes. Task body, child blocks, quote
prefixes, CRLF, and final-newline state follow the existing writer
rules. No metadata leaks into clean task descriptions or capture
previews.

**Counting, clearing, and storage.** Exact eligibility authorizes counting
only for an exact due-Ready ROTTEN/RETURNED row: the pre-write queue holds
exactly one row with `entry.path === path`, `entry.line === editorLine + 1`,
`entry.originalMarkdown === rawLine`, `lane === 'ready'`, and
`tier in {'rotten','returned'}`. The sole increment helper is
`api.freshness.keepLine(line, dateText, { counted })` with its valid-prior-
`fresh < today` guard; Rust reads/clears/reports with no increment path
(the seed preserve-mode exception aside). A v5 provider with a missing or
throwing `keepLine` fails without writing, while pre-v5 falls back to the
uncounted old stamper. The event table:

- Alt+F / Alt+Shift+F on an exact due Ready target in `rotten`/`returned`
  stamps and increments once in the same write, unless an active decision
  is required.
- NEW, FRESH/early, Pending, Next, Blocked, Today-linked, tracker-tier
  (PROJECTS/REFERENCES) trackers, other excluded targets, and
  unresolved cache matches stamp through `keepLine` uncounted and
  preserve the streak.
- A repeated same-day keep preserves the streak and is byte-identical.
- Every other supported human stamp clears the streak, even same-day.
- Close/cancel preserves the streak on the closed line.
- Automation, hooks, randomize, and seed neither increment nor reset.
- Recurring and closed targets are refused.
- Trackers in the PROJECTS/REFERENCES tiers never `decide` and their
  explicit keeps are uncounted — like PROJECTS, REFERENCES behaves as
  a commitment tier that never asks. Dependency capture (`bob
  capture` `&`) is a generic stamp that clears `keeps`.

What Rust guarantees today:

- every generic human stamp (`stamp_fresh`, `set_refresh`,
  including same-day stamps) clears `keeps`;
- refusals (not a task, recurring, closed) write nothing, so the
  streak stays on the closed line;
- automation, hooks, randomize, and the seed neither increment nor
  reset — `seed.rs::stamp_change` stamps through the private
  preserve-mode primitive, so the cutover never silently resets a
  streak;
- capture's existing `stamp_fresh` consumers clear `keeps` through
  the same default.

No backfill: seeded dates, captured dates, and old `fresh` stamps are
not evidence of keeps. Git sync may lose an increment during conflict
recovery; this scalar is not an exact global event counter. Cache
uncertainty under-counts. Hand editing remains unobserved.

**Configuration.**

```yaml
freshness:
  decay:
    keeps: 3
    # enter: P2
```

`decay` absent, null, `true`, or `{}` means enabled with 3 keeps.
`false` keeps counting/display but never asks or skips. `keeps`
accepts an integer 0–999; 0 asks on every due Ready re-confirmation,
never NEW. `enter` is an optional nonempty configured priority
label; absent/null uses interval-aware entry. Invalid scalar types,
fractional/negative/out-of-range values, and malformed blocks follow
the current freshness config failure contract: Rust exit 2; plugin
defaults with `invalid` diagnostics. Priority config validity is
resolved against the existing priority loader, not a second
hard-coded P1–P4 table.

**Availability.** Decay decisions are available as soon as decay is
enabled and compatible plugins are loaded. There is no calendar gate,
replacement date, or counting-only period. On an exact, due Ready
ROTTEN/RETURNED task at the keep limit, a single Alt+F/Alt+Shift+F
press opens the consent card and writes nothing, while counted and
Task Link sessions skip those targets (`N needs a decision`) without
changing fresh/count. Below threshold they keep counting normally
through exact pre-write matching into `keepLine`, with `kept N×`
tails on the Fresh notice. A card still requires an explicit gesture,
never a timer write. `freshness.decay: false` remains the off-switch:
count and show pips, without cards or decision skips. New navigation
with an older ledger falls back to counted keeps; new ledger with
older or missing navigation shows pips without a card promise.

**Read-time decision flag.** `decide = enabled && lane
ready && tier rotten/returned && keeps >= limit`. The annotation
means a choice is due, not permission to execute an action. The shared
approved-decay action planner (`planFreshnessDecayCard` in
bob-navigation-hotkeys) turns that flag into stable previewed decisions —
P0 entry, a non-cancelling Not now, Less often steps, and dated
review-decision entries with `· kept N×` tails — as specified in
`docs/projects.md` ("Approved-decay decision planner"); the card interaction
itself (`FreshnessDecayCardModal` plus guarded commit adapters, batch
skipping, the leaf, and the capability guard) landed with the
decision-card phase.

## 3. Placement rule

The Rust helper is `stamp_fresh` / `set_refresh`; the JavaScript
ones are `api.freshness.stampLine` / `setRefreshLine` / `keepLine` in
bob-ledger-tools (`keepLine` is the sole increment helper and landed
with the ledger-marks phase). One helper per language, pinned by the
shared vectors below. Rust reads, clears, and reports `keeps`; the
seed's private preserve-mode primitive is the only Rust path that
keeps a streak while stamping.

1. **Scope.** The helpers handle Tasks' Dataview format only, which is
   the vault's format. `bob freshness` refuses to run when the vault's
   Tasks settings use another format. Task-line detection strips
   leading `>` quote markers first (up to three spaces before each
   `>`, one optional space after), matching both Rust parsers — a
   quoted task is still a task.
2. **The Tasks suffix.** Scan from the end of the line, the way both
   parsers do:
   - first an optional trailing ` ^id` (the `BLOCK_LINK` grammar);
   - then repeatedly whitespace plus one of:
     - a `[k:: v]` or `(k:: v)` field whose key is a Tasks key
       (`priority start created scheduled due completion cancelled
       repeat onCompletion id dependsOn`), whatever its value,
       following `trailing_inline_field`'s grammar;
     - a `fresh`, `refresh`, or `keeps` field;
     - a trailing tag (the `HASH_TAG_AT_END` grammar).
   - Never scan past the start of the task body. If the body begins
     with the global filter token `#task`, never scan past the end of
     that token.

   The **Tasks suffix** starts at the leftmost Tasks element of that
   run (a Tasks-key field, a tag, or `^id`). `fresh` / `refresh` /
   `keeps` fields at the run's left edge are not part of it.
3. **Canonical output.**
   - Remove every `fresh`, `refresh`, and `keeps` field on the line.
     Each removal also collapses the whitespace it leaves to a single
     space.
   - Rebuild as `head.trim_end() + " " + "[fresh:: D]" + ("
     [refresh:: N]" if a refresh value is kept) + (" [keeps:: K]" if
     a keeps value is kept) + (" " + suffix if there is a suffix)`.
   - The kept refresh value is the first valid existing one;
     `set_refresh` replaces or removes it. Generic stamps keep no
     `keeps` value (they clear the streak); the seed preserve-mode
     keeps the first valid existing one and omits zero.
   - The suffix bytes themselves are never changed.
4. **No churn.** If the canonical output equals the input byte-for-byte
   (the line is already stamped today and canonical), report `changed:
   false`.
5. **Refusals.** Return the line unchanged with a reason for:
   - a line that is not a task line;
   - a recurring task (a `repeat` field in the suffix): its recurrence
     resurfaces it, and Tasks would copy the stamp into the next
     occurrence;
   - done and cancelled tasks, which callers must not stamp.
6. **Invariance.** For every vector, the Tasks fields both Rust parsers
   extract are identical before and after: status, dates, priority,
   recurrence, `id`, `dependsOn`, tags, block ID. So are the Obsidian
   Tasks fields the JavaScript tests can check.

## 4. Evaluation

Computed at read time, never stored. `today` is the vault's local
calendar date (`BOB_NOW` in tests).

```text
in_scope(t)   = status type TODO ("[ ]") ∧ lane-visible ∧ ¬recurring
                ∧ ¬in a canonical daily note (YYYY/YYYYMMDD.md) ∧ ¬Today(t)
                  lane-visible = the NEXT/PENDING lane predicate on each side: not done,
                  not dependency-blocked, not under _templates or _conflicts,
                  no scheduled date after today, no #hide for ordinary tasks
                  and exact ^prj rows — except exact ^ref trackers, which
                  bypass only the #hide exclusion (see "Tracking review" below)
fresh(t)      = the latest valid `fresh` date on the line; none if there is none
                (malformed ⇒ ignored + lint; a future date ⇒ treated as none + lint)
state(t)      = NEW         if no fresh(t)
              | RESURFACED  if scheduled(t) exists ∧ fresh(t) < scheduled(t) ≤ today
              | ROTTEN      if today ≥ fresh(t) + interval(t)   (stamped Mon at 7 ⇒ due next Mon)
              | FRESH       otherwise
                (Ready only; lane rows keep a null state)
lane(t)       = pending  if status symbol "/"
              | next     if status symbol "*"
              | ready    if status type TODO ("[ ]")
              | none     otherwise (Blocked, closed, custom non-TODO)
walk_scope(t) = lane(t) ≠ none ∧ lane-visible ∧ ¬recurring ∧ ¬canonical daily note ∧ ¬Today(t)
                (lane-visible is the existing NEXT/PENDING predicate, unchanged;
                exact ^ref rows use the freshness-specific visibility above)
lane_interval = freshness.pending_interval (pending) | freshness.next_interval (next);
                default 1; false = that lane is not walked
interval(t)   = ^prj with project_interval: project_interval, source "project"
                | ^ref with reference_interval: reference_interval, source "reference"
                | tracker without a configured cadence: the Ready chain above
                | ordinary lane task with a walked lane: lane_interval, source "pending" | "next"
                otherwise unchanged: task refresh → note task_refresh → freshness.interval → 7
                (a tracker in any lane never uses the lane interval, so the
                weekly reminder never becomes a daily lane review; the line
                picker shows it too. A set tracker interval wins over all of
                it, including task/note/global/lane levels)
lane_due(t)   = walked ordinary lane ∧ (no fresh(t) ∨ today ≥ fresh(t) + lane_interval)
tracker_due(t)= tracker lane ∧ walk_scope ∧ (no fresh(t) ∨ resurfaced
                ∨ today ≥ fresh(t) + interval(t)); a disabled lane walk
                never disables tracker review
due_on(t)     = ordinary lane row: fresh(t) + lane_interval, or none when never stamped
                (tracker lane row: effective tracker due date, none when never
                stamped)
                RESURFACED: scheduled(t); ROTTEN/FRESH: fresh(t) + interval(t); NEW: none
due(t)        = in_scope(t) ∧ state(t) ≠ FRESH
tier(t)       = projects  if due ^prj (Ready or lane, actual lane retained;
                            even when its Ready state is NEW or RESURFACED)
              | references if due ^ref (Ready or lane, actual lane retained;
                            even when its Ready state is NEW or RESURFACED)
              | new       if lane ready ∧ state NEW (ordinary tasks only)
              | pending   if lane pending ∧ walk_scope ∧ lane_due (never a tracker)
              | next      if lane next ∧ walk_scope ∧ lane_due (never a tracker)
              | returned  if state RESURFACED
              | rotten    if state ROTTEN
              | none      otherwise
```

The queue holds every row with a tier, in tier order NEW →
PROJECTS → PENDING → NEXT → RETURNED → REFERENCES → ROTTEN. Within
each tier the order is:

| Tier     | Order within the tier                                    |
| -------- | -------------------------------------------------------- |
| new      | path ↑, line ↑ (unchanged)                               |
| projects | never-confirmed first, due_on ↑, created ↑, path ↑, line ↑ |
| pending  | never-stamped first, due_on ↑, created ↑, path ↑, line ↑ |
| next     | same as pending                                          |
| returned | due_on (= scheduled) ↑, created ↓, path ↑, line ↑        |
| references | never-confirmed first, due_on ↑, created ↑, path ↑, line ↑ |
| rotten   | interval ↑, due_on ↑, created ↓, path ↑, line ↑          |

A missing `created` always sorts after dated peers within its tier,
in both ascending and descending keys. The commitment tiers are new,
projects, pending, next, returned, and references — PROJECTS and
REFERENCES sit before the "Commitments done" boundary, so meeting the
upkeep budget never signals that commitments are finished while
trackers remain. Rotten is upkeep. A PROJECTS or REFERENCES row never
acquires a decay decision for being rotten underneath;
ordinary rotten behavior is unchanged.

Counts keep state and tier distinct:

- `due`, `new`, `resurfaced`, `rotten`, and `fresh` count evaluated
  Ready states over the full review universe (`due = new +
  resurfaced + rotten`), including eligible Ready trackers once each.
  Pending/Next rows retain null state and never contribute.
- `by_tier` counts the actual full queue with all seven machine tier
  keys (zeroes for empty tiers); `walk = sum(by_tier.values())`,
  before any `--limit`.
- `pending_due`, `next_due`, `projects_due`, and `references_due`
  each equal their tier count, for symmetry.
- `refreshed_today` keeps its meaning.
- New count `upkeep_today`: tasks of any status outside `_templates`
  / `_conflicts` whose `fresh` equals today and whose status symbol is
  neither `/` nor `*`.
- `budget_met = budget set ∧ upkeep_today ≥ budget ∧ new == 0`
  (unchanged raw formula; outstanding commitment tiers take
  precedence in notices/status mode).

Every `✓` meter displays `upkeep_today`: `✓ N today`, or `✓ N/B
today` with a budget.

RESURFACED beats ROTTEN when both hold: a deferral that returned is
due as soon as it returns. The tickler makes a short deferral (for
example a P1 roll of 2–7 days) due as soon as it returns, without any
hooks write. Tiers never feed buckets or chips: `state`/`bucket` and
the partition text below are unchanged.

**Buckets.** The stable read-time bucket contract for dashboard
gating:

```text
bucket(t)     = new     if state(t) = NEW
              | rotten  if state(t) = RESURFACED or ROTTEN
              | null    if state(t) = FRESH or null (out of scope)
```

READY applies the visible TODO Ready pool plus `bucket !== "new"
&& bucket !== "rotten"` — never `state === "fresh"`, which would
lose freshness-exempt tasks. A null bucket alone never proves a task
is Ready. With the supported plugin and a ready cache, the visible
pool partitions as `B = NEW ∪ RETURNED ∪ ROTTEN ∪ READY`, pairwise
disjoint (RETURNED is bucket rotten with state resurfaced).

Bucket conformance: S1 maps to `new`; S3, S4, and S11 map to
`rotten`; S2, S5, S7, and S12 map to null; every S13 row maps to
null.

**Tracking review (projects and references).** The freshness walk
also reminds Bryan to replenish surfaced projects and to review
unfinished reading references, in one consistent contract on both
sides including the `]s` / Alt+Shift+F walk: NEW → PROJECTS →
PENDING → NEXT → RETURNED → REFERENCES → ROTTEN.

- Identity is the parsed, exact trailing block ID `prj` or `ref` on
  a real task. Tags alone, `^prj-extra`, description text, and
  `[[x#^prj]]` links/embeds are never identities; legacy lines
  without the cosmetic `#prj`/`#ref` tag stay supported.
- A `^prj` row is reviewed exactly when it does not carry `#hide`.
  `bob projects sync` owns that tag: it removes `#hide` when a
  project has no unhidden open tasks and no open sub-projects, and
  adds it back otherwise — so the evaluator never counts open tasks
  and never reads the note's `scheduled` frontmatter. A hidden
  `^prj` is out of scope (null state/bucket/tier), like any hidden
  task. If a visible `^prj` sits in a note that has open tasks
  because sync has not run yet, it is reviewed; sync runs every 15
  minutes, so the window is small.
- A `^ref` row keeps the `#hide` bypass: the conventional `#hide`
  tag is allowed without changing global lane queries, dashboard
  visibility, task tags, or project/highlights sync rules. Every
  other exclusion still applies (unsupported/closed/blocked status,
  dependency blocking, recurring, template/conflict path, canonical
  daily note, Today membership, future inline scheduling); ordinary
  hidden tasks remain out.
- The checkbox stays authoritative: done/canceled trackers
  disappear. Missing optional type/tag metadata never disables an
  exact anchor, and no task is created for a note missing its
  tracker. Creation, import, and sync never auto-confirm; existing
  confirmation dates are preserved.
- A visible `^prj` reviews on its tracker cadence
  (`project_interval` when set, otherwise the normal Ready interval
  chain `refresh` → `task_refresh` → `freshness.interval` → 7), due
  in PROJECTS even when its Ready state is NEW or RESURFACED, with
  ordinary inline-`scheduled` RESURFACED handling for Ready rows. A
  `[*]`/`[/]` `^prj` keeps its lane (with null state) and the same
  cadence in PROJECTS; a disabled lane walk never disables it.
  Trackers never decide and never count keeps.
- Every due in-scope `^ref`, in any lane, walks in REFERENCES. That
  includes never-confirmed Ready references, which no longer enter
  NEW. Lane rows keep their actual lane and a null state/bucket,
  like lane `^prj`; Ready references keep their Ready
  `state`/`bucket` unchanged (NEW, RESURFACED, ROTTEN, FRESH) and
  only the tier changes. The cadence is `reference_interval` when
  set, otherwise the Ready chain — never the lane interval. A lane
  reference is due when never stamped or when `today ≥ fresh +
  interval`. `pending_interval: false` / `next_interval: false` no
  longer disable reference review. Within REFERENCES the order is
  the same as PROJECTS: never-confirmed first, then `due_on` ↑,
  `created` ↑, path ↑, line ↑.

PR/RF conformance (fixed local dates; default interval 7, plus
config 10, note 14, task 3 for precedence, plus project 1 and
reference 3 for tracker precedence): a visible unstamped `^prj`
lands in PROJECTS once, never NEW; a hidden `^prj` stays out of
scope; stamps today/6-days-ago suppress while 7-days-ago and older
surface with accurate due metadata (1-day project and 3-day
reference cadences when set); a visible `^prj` in a note with open
tasks or a future frontmatter schedule is still reviewed, with no
lint; lane trackers keep their lane with null state and the Ready
chain (or the tracker cadence when set); disabled lanes still
review both tracker tiers; absent tracker keys preserve the prior
chain and ordinary tasks ignore both keys; an unstamped Ready
`^ref` walks in REFERENCES (never NEW) with its NEW state and
bucket intact; lane `^ref` rows walk in REFERENCES with null state
(never their lane tier); a RESURFACED Ready `^ref` walks in
REFERENCES and never decides, even at the keep limit; Today,
recurring, daily, template/conflict, blocked, and future-inline
rows stay excluded from review scope; tag-only/near-match/embedded
links never qualify; the seven-tier order holds with stable ties
and `walk = sum(by_tier)`; counts, JSON, human output, status bar,
`limit=1`, and the upkeep budget show no duplicate totals, with the
header and status bar reading tier counts; hidden `^ref` rows join
full review while the visible pool, lane, and capacity counts stay
unchanged; neither tracker tier ever decays.

**Machine vocabulary (schema 8).** Human output, help, and docs
say `rotten`, and so does the machine contract since the vocab-rotten
migration published JSON schema 2: `state: "rotten"`,
`counts.rotten`, and `freshness.rotten_daily_budget`. Each JSON queue
row still carries the `bucket` field (`"new"`, `"rotten"`, or null).
Schema 8 drops `config.decay.active_from` / `active`: decay
decisions are available as soon as decay is enabled, with no calendar
gate. Schema 7 adds the `references` walk tier between `returned` and
`rotten`, the seven-key `counts.by_tier` histogram,
`counts.references_due`, and the `^prj` hide gate (visible `^prj`
rows review on sync's `#hide` alone; the `project_scheduled_invalid`
lint is gone). Schema 6 adds `config.project_interval` /
`config.reference_interval` (number or null, null means inherit) and
the `project` / `reference` interval sources. Schema 5 added the
`projects` walk tier with `counts.projects_due` and the six-key
`counts.by_tier` histogram (`walk` sums it), and decoupled state
totals from tier totals as above; the shared seed envelope version
advances with each, seed behavior unchanged.
Likewise bob-ledger-tools uses the `"rotten"` state string under
freshness namespace v6 (`api.freshness.version === 6` with the
explicit `trackerReview` capability plus the explicit
`referenceReview` capability, which tells consumers the queue may
carry `references` entries; top-level api stays v3). Date-independent
decide/config landed in v6; counting through `keepLine` remains
available from v5.
Dashboard `freshness.reviewModel()` NEW/ROTTEN chips project the
same memoized evaluated states onto the existing visible Ready
pool — hidden review-only rows never feed a badge for a section
that excludes them — while the status bar footer, `]s`, and the CLI
use the full review counts. The desktop footer shows only nonempty
walk groups in NEW → PROJECTS → PENDING → NEXT → RETURNED →
REFERENCES → ROTTEN order, splitting RETURNED from ROTTEN so the
commitment/upkeep boundary is visible; dashboard ROTTEN chips still
fold RETURNED plus ROTTEN. It appears only while a trustworthy
nonempty queue remains and hides entirely when the queue is empty,
including a met upkeep budget or a nonzero today count. `Review r/N`
and `TIER i/M` are positions in the current queue; they never mean
how many tasks were completed in a review session. Compact cursor
context reuses the same presentation as the `]s` notice.

**One-release legacy budget key.** A config that still sets
`freshness.stale_daily_budget` keeps working for one release: when
only the old key is present it supplies the budget, and when both
keys are present the canonical `rotten_daily_budget` wins (including
an explicit null, which means budget off) while the old value is
ignored. Either case emits exactly one
`freshness_stale_daily_budget_deprecated` diagnostic per loaded
config — in `bob freshness` human/JSON warnings and in the plugin
lints — without marking the config invalid. An invalid value for the
selected key is still a config error.

Line numbers are 1-based in JSON and docs. Tasks' `lineNumber` is
0-based, so convert it.

**Lints:** `fresh_malformed`, `fresh_future`, `fresh_duplicate`,
`fresh_misplaced` (a `fresh`/`refresh`/`keeps` inside the Tasks
suffix; the next stamp repairs it), `refresh_invalid` (per task),
`keeps_invalid` (a `keeps` value that is not a decimal integer 1–999;
ignored) and `keeps_duplicate` (more than one `keeps` field; the
first valid one wins), and `task_refresh_invalid` (per note),
`freshness_stale_daily_budget_deprecated` (once per loaded config
when the removed `stale_daily_budget` key is present; see above).
`today_link_unresolved` passes through unchanged from the Today
engine.

## 5. Who stamps

**Rule:** a supported human gesture that already rewrites an open
task's line stamps that task in the same write, as the last
transformation of the line, if the task is still open and
non-recurring afterwards. Stamps apply in every lane, because a Next,
Pending, or Blocked task may come back to Ready later. A stamp dated
today on a canonical line is a no-op.

| Surface                | Stamps | Never stamps |
| ---------------------- | ------ | ------------ |
| bob-navigation-hotkeys | Alt+F and Alt+Shift+F stamp through `api.freshness.keepLine` (counted only under exact eligibility); Alt+N commit and release; the Ctrl+Shift+P Task Card (`1`–`4`, `0`, Enter/Schedule, `b`, `f`, `x`, Ctrl+D) and the stages its actions open, in single, counted and Task Link mode; each open task moved by Ctrl+Shift+M; the Ctrl+Enter recommended roll and decay; decision-card outcomes write through the existing writers (Not now and levels through the priority writer, Less often through set-refresh, Reword through the generic stamp, Drop through the cancel row, which never stamps) | the cancel row (including the decay cancel), project-frontmatter edits, create-project-note-from-task, the `!` transclusion toggle (a pure toggle that never rewrites a task line) |
| task-status-cycler     | Alt+[ / Alt+] (including counted and transcluded targets) when the result is an open status, including leaving Blocked by hand; Ctrl+Enter reopening a done task | closing (done or cancelled), Ctrl+Shift+] bullet → `#task` (that is creation), the dependency-ID normalizer, `recoverBlockedDependents` |
| block-id-prompt        | Ctrl+Shift+Enter and `^^` when they rewrite the task line (Ready/Blocked → Next, a new block ID) | unlink, Task Link removal, Ctrl+6 rename |
| `bob capture`          | `plan_task_link` (the link direction of `@route+id!`, Ensure Next, solo `@route:id` / `^route:id`, link-then-close), the `=x` rows that set `[/]`, and an `&note:id` dependency capture that edits an existing open dependent (its Depends-On line, derived fields, or Blocked status) | new tasks on any route (including new tasks with `&` prerequisites), `=x` complete, unlink, start rows, sub-bullets, prerequisite target-ID and lane promotion edits, an unchanged repeat `&` |
| Automation             | — | hooks, `projects sync`, `randomize`, `gkeep pull`, `highlights`, `move-done-tasks`, `nightly`, `vault-sync`, `capture-task-id` |
| `bob freshness seed`   | the one-time cutover (a documented exception) | — |
| Hand editing           | — | not monitored; edit, then press Alt+F |

Deferrals: scheduling a task into the future stamps it, and it becomes
Blocked. When it returns it is RESURFACED. Automatic Blocked → Ready
returns through the hooks never stamp.

Placement is the exception to the copy-small-helpers rule: nav,
task-status-cycler, and block-id-prompt call
`api?.freshness?.stampLine?.(line, dateText) ?? line` on
bob-ledger-tools (api `version >= 3`), and nav calls
`api.freshness.keepLine` (v5) for explicit keeps, with a source comment at each
call site. A missing stamp only means Bryan sees the task once more;
a misplaced stamp hides Tasks fields — so the risky part lives in one
place, and when ledger-tools is absent or old the gesture simply
doesn't stamp. Clicking a freshness mark only reveals the raw field
for editing and never stamps.

Before Alt+F or Alt+Shift+F stamps a validated Pending (`[/]`) target,
it asks once for an optional Work Log summary. In a counted or Task Link
batch, only the Pending targets receive the shared summary; every other
accepted target still refreshes. Enter on an empty summary still keeps
the task and writes no Work Log entry. Escape cancels the refresh, so the
task stays due and Alt+Shift+F does not advance.

## 6. Review ritual

**Morning, about 10 minutes once the lanes are at their caps:**

1. Run `bob gkeep pull`.
2. Use `]s` / Alt+Shift+F through NEW → PROJECTS → PENDING → NEXT →
   RETURNED → REFERENCES until the notice says **Commitments done**.
   `N]s` / `N[s` move N entries along that same queue and wrap with
   the existing notice, while `[S` / `]S` stay the first and last
   entries. NEW is never
   capped or skipped. In PROJECTS, replenish the empty project with
   Alt+Shift+F; stamping a project advances to the correct next entry
   with the block ID and `#hide` preserved. In REFERENCES, confirm
   the reference still needs reading. In the lanes, ask "still in this
   lane?": keep with Alt+Shift+F (Pending asks for an optional Work Log
   summary; blank Enter still keeps, while Escape leaves the task due),
   do it today with Ctrl+Shift+Enter,
   release with Alt+N. For a returned deferral, "not now" is a
   priority roll, not Alt+F.
3. Start the highlight.
4. Clear CROWDED to 0 (split, sequence, defer, drop via `bob ready`).
   This does not depend on how far ROTTEN review got.
5. Then, or later, do ROTTEN upkeep until 0 or the budget. It is fine
   to stop partway.

The Obsidian footer keeps a condensed version of that `]s` notice
visible while the cursor is on a review task, and the `]s next` hint
otherwise. Wrap and boundary preambles stay transient in the notice.

**First walk:** release the lanes to their caps.

**Weekly:** if more than about 90% of NEXT reviews end in keep,
lengthen `next_interval` to 2–3 and leave Pending at 1.

Review outcomes, one key each (every row except "edit" stamps by
itself): still right (Alt+Shift+F or Alt+F); see it less often
(Ctrl+Shift+P `f`, then 14 / 30 / 90); not now (Ctrl+Shift+P
`1`–`4`); do today (Ctrl+Shift+Enter / Alt+N); route to a project
(Ctrl+Shift+M); drop (Ctrl+Shift+P `x`); sequence (Ctrl+Shift+P `b`);
wording wrong (edit, then Alt+F).

A due at-limit task's Alt+F opens the decision card (Not now / Less
often / Reword / Drop / Keep) as soon as compatible plugins are
loaded. Counted and Task Link sessions skip such tasks with
`N needs a decision`. See `docs/projects.md` "Approved-decay
decision planner".

## 7. `bob freshness`

Headless review queue (`bob freshness list`, human and JSON) and a
guarded, idempotent, staggered cutover seed (`bob freshness seed`)
that aborts on any parse change.

Running `bob freshness` with no subcommand runs `list`.

```bash
bob freshness [-b|--bob-dir DIR] [-f|--format human|json] [-l|--limit N]
bob freshness list [-b|--bob-dir DIR] [-f|--format human|json] [-l|--limit N]
bob freshness seed [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-f|--format human|json]
```

`-b/--bob-dir` selects the vault (default `BOB_DIR`, then `~/bob`).
`list` options: `-f/--format human|json` (default `human`) and
`-l/--limit N` (queue rows only; counts always cover the whole
vault). Human output is colored only on a TTY:

```text
bob freshness · Thu 2026-10-08 · every 7d · pending 1d · next 1d · keeps 3

  REVIEW 68 due · 1 new · 2 projects · 10 pending · 15 next · 14 returned · 3 references · 26 rotten · ✓ 12 today

  NEW 1
    gkeep_inbox.md:14   Pick up our daughter    created 2026-09-30
  PROJECTS 2
    home.md:10          Replenish home          Empty project · never confirmed · every 7d (default)
    work.md:44          Staff the launch        Empty project · due today · fresh 2026-10-01 · every 7d (default)
  PENDING 10
    sase.md:40          Land the epic           never confirmed · created 2026-09-20
    work.md:12          Ship the report         due today · fresh 2026-10-07 · every 1d (pending)
  NEXT 15
    …
  RETURNED 14
    b.md:40             Week habits             returned · scheduled 2026-10-07 · fresh 2026-10-05
  REFERENCES 3
    lib/books/paper.md:8  Read the paper        Reference · due today · fresh 2026-10-01 · every 7d (reference)
  ── commitments done above · upkeep below ──
  ROTTEN 26
    d.md:1              Water the herbs         due today · fresh 2026-10-07 · every 1d (task)
    a.md:2              Rename queue input      rotten 3d · fresh 2026-09-28 · every 7d (note)
```

- The `REVIEW N due` total is `walk`.
- Human vocabulary says "returned"; the machine `state` stays
  `resurfaced`.
- A disabled lane shows `pending off` in the header.
- The header always shows the keep threshold: `keeps 3` when enabled,
  `keeps 3 · decay off` with decay off, and `keeps 0 · asks every
  review` for a zero threshold. Only a capable installed card
  promises `next review asks`.
- Human rows show `kept N×` when the streak is nonzero and
  `· decide` where the read-time choice is due.
- Each tier heading carries its count and is omitted when empty.
- The dim divider appears only when rows exist on both sides.
- Lane rows overdue by `n ≥ 1` days read `{n}d overdue`.
- With a budget, the meter reads `✓ 12/15 today`.
- `--limit` truncates rows, never counts.
- Lints go last, unchanged.

`text` is the clean description; queue `line` numbers are 1-based.
The seed was a one-time cutover and must not be re-run.

**Fallback.** With a missing, old, or throwing freshness API, dash
keeps legacy READY visibility and counts, NEW and both rotten groups
render empty, and NEW/ROTTEN badges show `–` (never zero). Native
`bob query` has no Obsidian `app.plugins`, so NEW/ROTTEN are empty
and READY is ungated there; `bob freshness list` is the headless
review interface.

The JSON contract is `schema_version: 8` with `ok`, `date`,
`config` (`interval`, `pending_interval` / `next_interval` as a number
or `false`, `project_interval` / `reference_interval` as a number or
null (null means inherit), `rotten_daily_budget`, plus normalized
`decay` with `enabled`, `keeps`, `enter` (label or null)),
`counts` (`due`, `new`,
`resurfaced`, `rotten`, `fresh`, `pending_due`, `next_due`,
`projects_due`, `references_due`, `by_tier` (all seven tier keys),
`walk` (`= sum(by_tier)`), `decide`, `refreshed_today`,
`upkeep_today`, `budget`, `budget_met`; counts always cover the whole
vault regardless of `--limit`), `queue` (each with `rank`, `tier`
(`new` | `projects` | `pending` | `next` | `returned` |
`references` | `rotten`), `lane` (`ready` | `pending` | `next`),
`state`, `bucket` (`"new"`, `"rotten"`, or null — lane rows carry
`state: null` and `bucket: null`, as does a hidden `^prj`), `path`,
`line`, `block_id`,
`status_symbol`, `text`, `created`, `fresh`, `interval`,
`interval_source` (`task` | `note` | `config` | `default` |
`pending` | `next` | `project` | `reference`), `due_on`,
`days_overdue`, `keeps`, `decide`), and `warnings` (`code`, `path`,
`line`, `message`). `decide` means a choice is due, not permission
to execute an action. No new CLI subcommands or options. Human
section counts, the REVIEW summary, status-bar walk totals, per-tier
ranks, and commitment-boundary logic use tier counts, not state
counts. The Obsidian footer omits zero-count groups and hides when
the walk is empty; CLI human output is unchanged. A NEW project
counts once in PROJECTS, and `--limit` only
truncates rows. A NEW-state Ready reference walks in REFERENCES, not
NEW, on the reference cadence in any lane.

`seed` options: `-d/--dry-run`, `-F/--force`, `-f/--format
human|json`. Ready tasks without a valid `fresh` are grouped by note
and bin-packed largest-note-first into 7 buckets; a note bigger than
`ceil(total / 7)` splits into consecutive line-order chunks, and ties
break by path, then bucket index. Bucket `k` (1–7) lands on `today −
7 + k`, raised per task to `max(bucket date, today − interval(t) +
1, scheduled(t) when due)` so nothing is due on cutover day and
nothing arrives RESURFACED, and clamped to today. Every other eligible open,
non-recurring task without a valid stamp gets today, including Today-linked
tasks. Tasks tagged `#hide` and tasks in daily notes, `_templates`,
`_conflicts`, or dot-directories are excluded. "Without a valid stamp"
includes missing, malformed, and future-dated `fresh` values.

If candidates remain, the seed refuses when any eligible task already
carries a valid `fresh` dated before today (unless `--force`). `--force`
bypasses that refusal; it does not rewrite existing valid stamps. A rerun
with no candidates reports zero newly stamped tasks, even if bucket stamps
are older than today. Later captures remain unconfirmed and should be reviewed
normally.

The seed aborts without writing notes when any changed task line would parse
differently under either Rust parser. A writing run then re-reads all touched
files before the first write and refuses if one changed; `--dry-run` stops
after planning and parse validation, without that file recheck. Each file is
written through a temp file plus rename. The JSON contract is
`schema_version: 8` with `ok`, `date`, `dry_run`,
`stamped` (`ready`, `other`), `buckets` (`fresh`, `due_on`, `count`,
`notes`), `skipped` (`already_stamped`, `recurring`,
`out_of_scope`), `files`, and `warnings`. The shared schema constant
also moves the `seed` envelope to 8, with seed content unchanged. Seed
candidate selection is unchanged, and list stays read-only. The `buckets`
dates describe the initial distribution: `fresh` is the unadjusted bucket
date, and `due_on` adds the global interval. Per-task adjustments described
above can make the written stamps and actual due dates differ from these
summary dates; the report does not list each task's final stamp.

The pre-write guards abort without note changes, but a filesystem failure
during the multi-file write can leave earlier files stamped. The error reports
the files already written; use the vault's Git history to recover them before
retrying. This seed does not use capture's batch rollback mechanism.
It does not lock the vault or recheck files between that pre-write validation
and their replacement; avoid editing the affected notes during the seed.

Exit codes: 0 on success; 1 for I/O errors and seed refusals; 2 for
an invalid `freshness:` block or a non-Dataview task format.

## 8. Surfaces

| Surface | Phase |
| ------- | ----- |
| `bob freshness` | fresh-cli (landed: `list` and `seed` in `src/native/freshness/`); tiered walk (schema 8: `list` walks NEW → PROJECTS → PENDING → NEXT → RETURNED → REFERENCES → ROTTEN with lane intervals for ordinary tasks and tracker cadences for `^prj`/`^ref`; decay config is `{enabled, keeps, enter}` with no calendar gate) |
| `bob capture` | capture-stamps (landed: plan_task_link + `=x` close stamp via `stamp_fresh`; existing-dependent `&` dependency stamp via the same helper; capture never stamps trackers) |
| bob-ledger-tools | ledger-freshness (landed: api v3 `api.freshness` + status bar in 1.8.0; tracking review under namespace v6 with the `trackerReview` capability; persistent review footer with additive `reviewEntryView`) |
| bob-navigation-hotkeys | nav-review, nav-stamps (landed: Alt+N + Ctrl+Shift+P/Ctrl+Shift+M/! stamping + refresh row in 1.44.0; PROJECTS/REFERENCES tier walk with `trackerReview`/`referenceReview` capabilities and legacy v3/v4 fallback; notices feature-detect `reviewEntryView`) |
| task-status-cycler | cycler-link-stamps (landed: Alt+[/Alt+] + Ctrl+Enter reopen stamping in 1.18.0) |
| block-id-prompt | cycler-link-stamps (landed: Ctrl+Shift+Enter + ^^ stamping in 1.16.0) |
| `rotten.md` (aliases `Review`, `Freshness review`, `Rotten Tasks`) | dash-gating (landed: live summary plus always-present RETURNED and ROTTEN groups; tasks stay in source notes, rows are click-through views) |
| `dash.md` | dash-gating (landed: TODAY → NEW → PENDING → NEXT → READY sections; grouped Work / Review / Browse navigation with NEW/PENDING/NEXT/READY/CROWDED/BLOCKED/ROTTEN/TODAY plus PROJECTS/REFERENCES chips; gated READY with whole-lane tooltip; CROWDED via `noteReady` v1 opening crowded.md; PROJECTS/REFERENCES via `dashboardCollections` v1 opening dash_projects/dash_references with Base-contract guards) |
| `crowded.md` + `bob-ready-notes` + heading chips | per-note Ready cap rollout (landed: Crowded Notes page, ranked-bar code block, live `ready n/cap` chips on each note's `## Tasks` heading) |
| freshness mark | fresh-mark (landed: bob-ledger-tools 1.10.0 Live Preview + rendered views) |

## 9. Placement conformance examples

D = `2026-10-08`. Each gives the input line, the expected line, and
`changed`. The bob-ledger-tools JavaScript tests use these verbatim.

- **P1 bare:** `- [ ] #task Buy milk` →
  `- [ ] #task Buy milk [fresh:: 2026-10-08]` (`changed: true`)
- **P2 created:** `- [ ] #task Buy milk [created::2026-09-29]` →
  `- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]`
- **P3 block ID only:** `- [ ] #task Pick up Abby ^pickup` →
  `- [ ] #task Pick up Abby [fresh:: 2026-10-08] ^pickup`
- **P4 interleaved suffix:**
  `- [ ] #task Plan trip [created::2026-08-26] #hide [priority:: high] ^trip` →
  `- [ ] #task Plan trip [fresh:: 2026-10-08] [created::2026-08-26] #hide [priority:: high] ^trip`
- **P5 unknown field stays left:**
  `- [ ] #task Read X [[#^h-8bac|🔖]] [h:: e629] [created::2026-08-28]` →
  `- [ ] #task Read X [[#^h-8bac|🔖]] [h:: e629] [fresh:: 2026-10-08] [created::2026-08-28]`
- **P6 spacing:**
  `- [ ] #task Rahway  [created:: 2026-07-15]  [scheduled:: 2026-08-10] ^rahway` →
  `- [ ] #task Rahway [fresh:: 2026-10-08] [created:: 2026-07-15]  [scheduled:: 2026-08-10] ^rahway`
  (the head collapses to one space; the suffix bytes are untouched)
- **P7 restamp:**
  `- [ ] #task Buy milk [fresh:: 2026-10-01] [created::2026-09-29]` →
  the same line with `[fresh:: 2026-10-08]`
- **P8 same day:**
  `- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]` →
  byte-identical, `changed: false`
- **P9 misplaced:**
  `- [ ] #task Buy milk [created::2026-09-29] [fresh:: 2026-10-01]` →
  `- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]` (the
  reader reports `fresh_misplaced` on the input)
- **P10 duplicates:**
  `- [ ] #task A [fresh:: 2026-09-01] B [fresh:: 2026-09-20] [created::2026-09-01]` →
  `- [ ] #task A B [fresh:: 2026-10-08] [created::2026-09-01]` (the
  reader reports `fresh_duplicate` and uses 2026-09-20)
- **P11 refresh follows fresh:**
  `- [ ] #task Rename queue input [refresh:: 14] [created::2026-09-10] [priority:: low]` →
  `- [ ] #task Rename queue input [fresh:: 2026-10-08] [refresh:: 14] [created::2026-09-10] [priority:: low]`
- **P12 set / clear refresh:** `set_refresh(P2 output, 30)` →
  `- [ ] #task Buy milk [fresh:: 2026-10-08] [refresh:: 30] [created::2026-09-29]`,
  and `set_refresh(…, none)` removes it
- **P13 recurring refused:**
  `- [ ] #task Water plants [repeat:: every week] [created::2026-09-01]` →
  unchanged, refused as `recurring`
- **P14 global-filter floor:** `- [ ] #task #hide ^x` →
  `- [ ] #task [fresh:: 2026-10-08] #hide ^x`, and
  `- [ ] #task #prj Ship it #hide ^prj` →
  `- [ ] #task #prj Ship it [fresh:: 2026-10-08] #hide ^prj`
- **P15 parenthesized field:** `- [ ] #task Call mom (created:: 2026-09-01)` →
  `- [ ] #task Call mom [fresh:: 2026-10-08] (created:: 2026-09-01)`
- **P16 indented Blocked:**
  `\t- [?] #task Deferred [created::2026-09-01] [scheduled:: 2026-10-20]` →
  `\t- [?] #task Deferred [fresh:: 2026-10-08] [created::2026-09-01] [scheduled:: 2026-10-20]`
- **P17 done refused:** `- [x] #task Old [completion:: 2026-10-01]` →
  unchanged, refused as `closed`
- **P18 quoted:** `> - [ ] #task Quoted [created::2026-09-01]` →
  `> - [ ] #task Quoted [fresh:: 2026-10-08] [created::2026-09-01]`
  (quote markers are stripped for task detection only, mirroring the
  vault scanner; the `>` bytes stay in place, and more than three
  leading spaces before `>` is still not a task)

## 10. State conformance examples

Today `2026-10-08`, the default config (interval 7, both lanes
1), and a Ready, visible, non-recurring task in `a.md` unless noted.
The bob-ledger-tools JavaScript tests use these verbatim.

- **S1 new:** no `fresh` → `new`
- **S2 fresh:** `fresh 2026-10-02` → `fresh`, `due_on 2026-10-09`
- **S3 boundary:** `fresh 2026-10-01` → `rotten`, `due_on 2026-10-08`,
  `days_overdue 0`
- **S4 overdue:** `fresh 2026-09-20` → `rotten`, `due_on 2026-09-27`,
  `days_overdue 11`
- **S5 task beats note:** `[refresh:: 14]`, note `task_refresh: 3`,
  `fresh 2026-10-01` → `fresh` (task source, due 2026-10-15)
- **S6 note beats config:** note `task_refresh: 3`, `fresh 2026-10-05`
  → `rotten` (note source)
- **S7 config:** `freshness.interval: 10`, `fresh 2026-10-01` →
  `fresh` (config source, due 2026-10-11)
- **S8 invalid overrides fall through:** `[refresh:: 0]` →
  `refresh_invalid`, and note `task_refresh: soon` →
  `task_refresh_invalid`; both fall through to config 7
- **S9 malformed:** `[fresh:: 2026-13-01]` → `new` + `fresh_malformed`
- **S10 future:** `[fresh:: 2026-10-09]` → `new` + `fresh_future`
- **S11 resurfaced:** `fresh 2026-10-05`, `scheduled 2026-10-07` →
  `resurfaced`, `due_on 2026-10-07`
- **S12 not resurfaced:** `fresh 2026-10-07`, `scheduled 2026-10-07` →
  `fresh`
- **S13 out of scope** (state `null`): a recurring task; a `[?]`
  with a future `scheduled`; `[*]`; `[/]`; `#hide`; `_templates/x.md`;
  daily note `2026/20261008.md`; a Ready task linked under today's
  open Pomodoro; a `[ ]` whose `dependsOn` names an open task.
  Note that `[*]` and `[/]` keep a null `state` and `bucket` but are
  in tier scope (see L1).
- **S14 queue order (rewritten).** Given NEW `b.md:3` and NEW `a.md:9`;
  ROTTEN due 2026-10-01 at `c.md:2`; RESURFACED due 2026-10-07 at
  `a.md:4`; ROTTEN due 2026-10-07 at `a.md:2`. The order is `a.md:9`,
  `b.md:3`, `a.md:4` (returned), `c.md:2`, `a.md:2`.
- **S15 counts (updated):** `refreshed_today` counts a `[*]` and an
  `[x]` stamped 2026-10-08, but not a Ready task stamped 2026-10-07;
  `upkeep_today` counts the `[x]` but not the `[*]`. With budget 15,
  15 upkeep and 0 new → `budget_met: true`; with 1 new → `false`.
- **Q1 (Bryan's example).** All four tasks are ROTTEN:
  - `d.md:1` `[refresh:: 1]`, fresh 10-07, created 09-01;
  - `c.md:1` fresh 09-28, created 09-04;
  - `b.md:1` fresh 09-28, created 09-01;
  - `a.md:1` fresh 09-30, created 09-01.
  Order: `d.md:1` (A), `c.md:1` (D), `b.md:1` (C), `a.md:1` (B).
- **Q2 (tier order beats path order).** NEW `e.md:1`; `[/]` `d.md:1`
  fresh 10-07; `[*]` `c.md:1` fresh 10-07; RETURNED `b.md:1` fresh
  10-05 scheduled 10-07; ROTTEN `a.md:1` fresh 09-20. Order: e, d, c,
  b, a, with tiers new, pending, next, returned, rotten.
- **L1.** A `[*]` stamped 10-08 is in no tier. A `[*]` stamped 10-07
  has tier `next`, `due_on` 10-08, `days_overdue` 0, interval 1 from
  source `next`, and null `state`/`bucket`.
- **L2 (lane overrides refresh).** A `[/]` with `[refresh:: 30]`
  stamped 10-07 has tier `pending` and interval 1 from source
  `pending`.
- **L3.** A recurring `[*]`, a Today-linked `[*]`, a `[*]` in
  `2026/20261008.md`, and a `#hide` `[*]` are each in no tier.
- **L4.** With `next_interval: false`, a never-stamped `[*]` is in no
  tier, and its interval falls back to the Ready chain (7, default).
  `next_interval:` null means 1.
- **L5 (lane order).** Five `[/]` tasks:
  - `z.md:9` never stamped, created 09-01;
  - `b.md:1` fresh 10-01, created 09-15;
  - `a.md:5` fresh 10-07, created 09-10;
  - `a.md:2` fresh 10-07, created 09-20;
  - `a.md:1` fresh 10-07, no created.
  Order: z.md:9, b.md:1, a.md:5, a.md:2, a.md:1.
- **R1 (RETURNED beats older ROTTEN).** RETURNED `b.md:1` (fresh 10-05,
  scheduled 10-07) comes before ROTTEN `a.md:1` (fresh 09-20, due
  09-27).
- **R2 (returned order).** `x.md:1` scheduled 10-06 created 09-01,
  `w.md:1` scheduled 10-07 created 09-05, `y.md:1` scheduled 10-07
  created 09-01. Order: x, w, y.
- **B1.** 20 lane stamps plus 5 Ready stamps today, budget 15 →
  `upkeep_today` 5, `refreshed_today` 25, `budget_met: false`. Adding a
  Blocked `[?]` and an `[x]` stamped today → `upkeep_today` 7.

## 10a. Keep-streak conformance vectors (K1–K13)

D = `2026-10-08`. The machine-readable form of every vector below —
plus the `C` config, `D` decide, and `B` boundary vectors — is
`tests/fixtures/freshness_keeps/vectors.json`, which both languages
cite. Rust runs the read/reset/placement cases and has no production
increment API; the JS side runs the increment vectors through
`api.freshness.keepLine`.

- **K1 absent:** no `keeps` → `keeps 0`, no lints.
- **K2 first valid:** `[fresh:: 2026-10-01] [keeps:: 2]` preserved
  (seed mode) →
  `- [ ] #task Rename queue input [fresh:: 2026-10-08] [keeps:: 2]`.
- **K3 refresh/keeps order:** `[keeps:: 2] [refresh:: 14]
  [fresh:: 2026-10-01]` on a line with a Tasks suffix canonicalizes
  to `fresh`, `refresh`, `keeps`, then the suffix.
- **K4 same-day preservation:** a canonical `[fresh:: 2026-10-08]
  [keeps:: 2]` line stamped in preserve mode is byte-identical
  (`changed: false`).
- **K5 generic stamp clears:** the same line stamped generically
  loses `keeps` (`changed: true`), even same-day.
- **K6 set-refresh clears:** `set_refresh(…, 30)` on a line with
  `refresh` and `keeps` writes `[fresh:: D] [refresh:: 30]` with no
  `keeps`.
- **K7 stale stamp clears:** `[fresh:: 2026-10-01] [keeps:: 1]`
  stamped generically → `[fresh:: 2026-10-08]`, no `keeps`.
- **K8 misplaced:** `[created::…] [keeps:: 2]` reads `keeps 2` with
  `fresh_misplaced`; the next stamp repairs it to canonical order.
- **K9 duplicates and invalid values:** two `keeps` fields →
  `keeps_duplicate`, first valid wins; `0`, negative, fractional,
  `1000`, and non-numeric values → `keeps_invalid` and report 0; an
  invalid value beside a valid one lints `keeps_invalid` and keeps
  the first valid value.
- **K10 ceiling and NEW:** `[keeps:: 999]` reads 999 (increments
  saturate at 999 on the JS side); a never-confirmed task reads
  `keeps 0`.
- **K11 refusals preserve:** done/cancelled and recurring lines are
  refused unchanged — the streak stays on the closed line, and there
  is no write.
- **K12 parser invariance:** for every vector, the Tasks fields both
  Rust parsers extract are identical before and after, exactly as §9
  requires for `fresh`/`refresh`.
- **K13 exact keys:** similarly named keys (`keep`, `Keep`,
  `keepsx`) are not `keeps`; the key match is exact and
  case-sensitive. Readers accept bracket and paren field syntax.

Config vectors C1–C13 pin `decay` normalization (absent, null,
`true`, `{}`, `false`, `keeps: 0`, fixed `enter`, and each invalid
shape). Decide vectors D1–D8 pin `decide`: at-limit Ready due rows
on every local day, below-limit Ready rows, RETURNED coverage, NEW
never, `keeps: 0` asking on every due Ready re-confirmation,
`decay: false` never asking, and lane rows never deciding. Boundary
vectors B1–B5 pin immediate availability on 2026-10-04, 2026-10-08,
2026-10-18, 2026-10-19, and 2026-10-20. Rows have carried
`keeps`/`decide` since schema 4, and the current schema is 8; counts
carry `decide`; `config.decay` reports normalized `enabled`,
`keeps`, and `enter`. CLI human rows show `kept N×` and `· decide`
where true; the header explains the threshold or the off state.
Counts cover the full queue regardless of `--limit`.

## 11. Display: the freshness mark

The freshness mark is display-only and lives only in bob-ledger-tools.
Nothing ever writes it: `[fresh:: YYYY-MM-DD]` stays the only stored
form, and Rust surfaces already strip inline fields
(`note_tasks::clean_description`). The mark is computed at render time
from the stamp, the interval, and the shared evaluator that also
drives `bob freshness list`, the status bar, Ctrl+Alt+J, and
`freshness.md`.

**Principles.** Store absolute, show relative. One glyph lifecycle,
borrowed from the status bar: `✓` when confirmed today, a lease ring
that drains as the task ages, `⟳` when due (Alt+F turns `⟳` back into
`✓`). Loud only when actionable: only tasks the evaluator says are
due get color and a capsule. Truthful or neutral: when the plugin
cannot identify the exact task, the mark shows only the neutral lease
and never a guessed "due"; non-canonical stamps keep their Dataview
pill, flagged as needing repair. Reversible and editable: the cursor
or a click reveals the raw text, source mode shows raw text, a session
toggle restores the old pills, and without bob-ledger-tools the vault
falls back to today's pills.

**Anatomy and tones.** A mark is `[glyph][label][interval?]`,
rendered at 0.8em in the interface font with tabular numerals, sitting
on the text baseline.

| Tone | When | Glyph | Color |
| ---- | ---- | ----- | ----- |
| `today` | age 0 on an open task | circle-check (full ring + tick) | `--task-status-next` green |
| `aging` | evaluator says FRESH, or the task is unresolved | lease ring: faint track + arc for the remaining lease | `--text-muted` |
| `due` | evaluator says ROTTEN or RESURFACED | `⟳` (rotate-cw) | `--color-orange`: the only loud tone (14% tinted capsule, 1px inset ring, weight 650) |
| `resting` | evaluator says out of scope (Next, In Progress, Blocked, linked today, …) or the task is closed | lease ring | `--text-faint`, quieter still |

Each tone is also distinguished by its glyph, and `due` by its
capsule, so the marks stay readable for color-blind users.

**Label, ring, and interval.** `ageDays = days(fresh → today)`, never
negative because future stamps get no mark. The label is `today` at
age 0, otherwise `{N}d` (`1d`, `7d`, `282d`); days are always the
unit, matching `bob freshness`. The interval is lane-aware per §4: a
lane task in a walked lane uses its lane interval, otherwise the
existing precedence (task `refresh`, then the note's `task_refresh`,
then `freshness.interval`, then 7). `remaining =
clamp((interval − ageDays) / interval, 0, 1)`, rounded to 4 decimals:
full on the day of confirmation, empty exactly when due (at 0 only
the faint track is drawn). The ring starts at 12 o'clock, drawn
clockwise for `remaining`. The interval suffix `/{N}d` appears only
when the effective interval source is `task`, and never on the `today`
tone; note, config, default, and lane intervals stay tooltip-only.
`freshnessMarkEveryPhrase` renders the `pending`/`next` sources as
` (pending lane)` / ` (next lane)`.

**Tooltip.** An `aria-label` with `data-tooltip-position="top"`,
lines joined with `\n`, never containing `::`. Dates use fixed English
names independent of locale (`Thu, Oct 1`; the year is appended only
when it differs from today's year: `Tue, Dec 30, 2025`). Relative
age: `today`, `yesterday`, or `N days ago`. `every …` reads `every N
days` (or `every 1 day`), plus ` (this task)`, ` (this note)`, or
` (config)` for those sources and nothing for the default, plus
` (pending lane)` / ` (next lane)` for the lane sources and
` (project)` / ` (reference)` for the tracker sources. Line 1:
`Confirmed today` at age 0, otherwise `Confirmed {date} ·
{relative}`. Line 2 is picked by resolution rather than tone: closed
or out of scope → `Not in the review queue: {reason}`; a due lane
row → `Daily {PENDING|NEXT} review due since {dueOn} · every …`; a
lane row stamped today → `Next review {fresh+interval} · every …
({pending|next} lane)`; a PROJECTS row → `Empty project` with its
confirmation/due detail and effective tracker interval (never a daily
lane review or a generic hidden-task exemption); a REFERENCES row →
`Reference` with the same shape on the reference cadence; ROTTEN →
`Due for review since {dueOn} ·
every …`; RESURFACED → `Resurfaced {scheduled}: scheduled after it
was confirmed`; FRESH or unresolved with the lease running → `Next
review {fresh+interval} · every …`; unresolved with the lease over →
`Review lease ended {fresh+interval} · every …`. Line 3, `due` tone
only: `Alt+F to confirm` (lane rows add the lane keep/release/today
keys per M9). Lane and tracker-tier rows with a tier get the `due`
tone and lane rows stamped today get the `today` tone. Lane tasks
outside the walk (Today, daily note, disabled lane, recurring,
hidden ordinary tasks) keep `resting` with the existing reasons —
but a disabled lane never takes trackers out of the walk. Reasons by
status symbol: `*` Next, `/` In Progress, `?` Blocked, `x`/`X` Done,
`-` Cancelled, any other non-space symbol `status [s]`; for `[ ]`,
the first that applies: `linked today`, `in a daily note`,
`recurring`, `in _templates or _conflicts`, `scheduled for {date}`,
otherwise `hidden or dependency-blocked`.

**Eligibility.** A Live Preview task line gets a mark only when
`freshnessTaskStatus(line) !== null` (quote-aware), the line has
exactly one `fresh` field and it is square-bracketed, and
`readFreshness(line, today)` yields a date with none of
`fresh_malformed`, `fresh_future`, `fresh_duplicate`, or
`fresh_misplaced`. Refresh folding: when the line's only `refresh`
field is square-bracketed, starts exactly one space after the `fresh`
field, and parses (1–365), it folds into the mark; otherwise it stays
a Dataview pill. Space folding: when the character before `[fresh::`
is a space, the decoration range starts at that space and the widget
restores the gap, so the mark wins over Dataview's widget by
position. Lines inside code and lines in source mode never get a
mark. Rendered views (a text node) skip the task-line and misplaced
checks, because Tasks may have split off the suffix: the node must
hold exactly one square-bracketed `fresh` field with a strict,
non-future date, with adjacent refresh folding as above; text inside
`code`, `pre`, `.bob-fresh-mark`, or `.dataview.inline-field` is
never touched.

**Resolution.** Exact first: a memo row with the same `path`, the
same 0-based line, and `rawLine === lineText` (Live Preview only),
modelled via `freshnessEvaluate`. Then consensus: every memo row in
`path` whose own mark source text equals this mark's source text;
when at least one candidate exists and all models are deep-equal, use
it. Otherwise unresolved: a neutral lease with tone `today` or
`aging`, the interval from the line plus `noteFreshnessRawFor(path)`
plus config, and `data-resolved="false"`. In Live Preview the line's
own status symbol always applies the closed override, even when the
memo is a moment behind.

**Interaction and surfaces.** Live Preview: the mark hides while any
selection range overlaps the field span (Dataview's inclusive rule),
so moving the cursor in shows the raw text; a mousedown on the mark
places the cursor at the start of the field and focuses the editor
(the mark never writes; Alt+F confirms); marks recompute on edits,
viewport and selection changes, the debounced freshness refresh, and
the date rollover, with widgets comparing a model key in `eq()` so an
unchanged mark never flickers. Rendered views (reading view, Tasks
query results such as `dash.md` and `freshness.md`, embeds, hover
previews) render when the host renders and refresh when it
re-renders. Repair flag: while marks are on,
`body.bob-fresh-marks` is set and any leftover `fresh`/`refresh`
Dataview pill in Live Preview is a non-canonical stamp, shown at full
opacity with a dashed orange border. Session toggle: the "Toggle task
freshness marks" command (default on) flips the marks and the body
class, refreshes every editor, and shows a Notice.

**Folded keep display (shipped ledger-tools 1.23.0/1.24.0).** From
`freshnessMarkModel`, the keep-pip and leaf rendering, and the tooltip
builder near the `Alt+F to decide` string (tests
`scripts/test-ledger-tools-freshness-keeps.cjs`,
`scripts/test-ledger-tools-freshness-decision-card.cjs`):

- Faint filled dots after the label: `--text-faint`, or subdued orange
  inside a due capsule, never green or red.
- Dot cap: 3 by default; the threshold itself for thresholds 1–2;
  overflow `+N`; the exact count in the accessible text.
- The leaf replaces `⟳` with `data-decide="true"` only when the nav card
  capability is present and decay is on.
- Folding: exactly one valid square-bracket `[keeps:: N]` one space after
  the folded fresh/refresh run is folded. Noncanonical fields keep the
  dashed repair pill.
- Selection or click reveals the whole raw span and never writes.
- Ambiguous consensus stays neutral, and closed or out-of-scope tasks show
  dots quietly without a leaf.
- Tooltip wording: `Kept N reviews in a row · Bob asks at L`, the
  `Alt+F to decide` hint, and counting-only/off wording otherwise
  (`Kept N reviews in a row` alone without the card capability,
  `· decay off` with decay off, `· Bob asks every review` for a zero
  threshold).

**Live verification (Bryan, in Obsidian).**

- Task lines in Live Preview show `✓ today`, the ring with `Nd`,
  and `⟳ Nd` in an orange capsule; no Dataview `FRESH` pill appears
  beside a mark.
- Moving the cursor into a mark, or clicking it, reveals
  `[fresh:: …]` for editing.
- Alt+F on a `⟳` task flips it to `✓ today` within about a second.
- Ctrl+Shift+P `f` → 14 shows `/14d` from the next day.
- `dash.md` and `freshness.md` Tasks results and reading view show
  marks; the freshness.md DUE group shows `⟳`.
- A hand-broken stamp (`[fresh:: 2026-13-01]`) shows the dashed
  repair pill.
- "Toggle task freshness marks" restores the old pills and back
  again.
- The marks look right in the light and dark themes.
- Metadata Menu does not double-decorate the field.
- Tooltips show their lines. If Obsidian collapses `\n`, record
  that as a follow-up rather than changing the format.

**Rejected alternatives.** Stored-syntax changes (emoji, compact
codes, stored relative ages) would leave Dataview's field index,
rot overnight, and rewrite both implementations plus ~540 vault
lines. CSS-only restyles cannot compute an age, lease, or review
state. Hiding `fresh` entirely loses the due signal. Lane-by-CSS
accents contradict the queue for `#hide`, linked-today, and
RESURFACED tasks; exact-or-neutral wins.

## 12. Mark conformance examples

Today `2026-10-08` (Thursday), config interval 7. Each M vector is a
Ready, visible, non-recurring task in `a.md` that the evaluator
resolves, unless noted. Tooltip lines are separated by `⏎` here; the
code joins them with `\n`. The bob-ledger-tools JavaScript tests use
these vectors verbatim.

- **M1 today:**
  `- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]`
  → text `[fresh:: 2026-10-08]`, foldSpace true; tone `today`, glyph
  `check`, label `today`, intervalLabel null, remaining 1. Tooltip:
  `Confirmed today ⏎ Next review Thu, Oct 15 · every 7 days`.
- **M2 aging:** `[fresh:: 2026-10-05]` → age 3, label `3d`,
  remaining 0.5714, tone `aging`, glyph `ring`. Tooltip:
  `Confirmed Mon, Oct 5 · 3 days ago ⏎ Next review Mon, Oct 12 ·
  every 7 days`.
- **M3 boundary:** `[fresh:: 2026-10-01]` → ROTTEN; tone `due`, glyph
  `refresh`, label `7d`, remaining 0. Tooltip:
  `Confirmed Thu, Oct 1 · 7 days ago ⏎ Due for review since Thu, Oct
  8 · every 7 days ⏎ Alt+F to confirm`.
- **M4 yesterday:** `[fresh:: 2026-10-07]` → label `1d`, remaining
  0.8571. Tooltip:
  `Confirmed Wed, Oct 7 · yesterday ⏎ Next review Wed, Oct 14 · every
  7 days`.
- **M5 folded refresh:**
  `- [ ] #task Rename queue input [fresh:: 2026-10-05] [refresh:: 14] [created::2026-09-10] [priority:: low]`
  → text `[fresh:: 2026-10-05] [refresh:: 14]`; label `3d`,
  intervalLabel `/14d`, remaining 0.7857. Tooltip:
  `Confirmed Mon, Oct 5 · 3 days ago ⏎ Next review Mon, Oct 19 ·
  every 14 days (this task)`.
- **M6 refresh today:** `[fresh:: 2026-10-08] [refresh:: 14]` → tone
  `today`, label `today`, intervalLabel null. Tooltip:
  `Confirmed today ⏎ Next review Thu, Oct 22 · every 14 days (this
  task)`.
- **M7 note interval:** note `task_refresh: 3`,
  `[fresh:: 2026-10-06]` → label `2d`, remaining 0.3333,
  intervalLabel null. Tooltip:
  `Confirmed Tue, Oct 6 · 2 days ago ⏎ Next review Fri, Oct 9 ·
  every 3 days (this note)`.
- **M8 resurfaced:**
  `- [ ] #task Week habits [fresh:: 2026-10-05] [scheduled:: 2026-10-07]`
  → tone `due`, glyph `refresh`, label `3d`, remaining 0.5714.
  Tooltip:
  `Confirmed Mon, Oct 5 · 3 days ago ⏎ Resurfaced Wed, Oct 7:
  scheduled after it was confirmed ⏎ Alt+F to confirm`.
- **M9 (changed).** `- [*] #task Ship it [fresh:: 2026-09-20]`
  gives tone `due`, glyph `refresh`, label `18d`, remaining 0.
  Tooltip: `Confirmed Sun, Sep 20 · 18 days ago ⏎ Daily NEXT review
  due since Mon, Sep 21 · every 1 day (next lane) ⏎ Alt+F keep · Alt+N
  release · Ctrl+Shift+Enter today`.
- **M10 (changed).** `- [*] #task Ship it [fresh:: 2026-10-08]` gives
  tone `today`. Tooltip: `Confirmed today ⏎ Next review Fri, Oct 9 ·
  every 1 day (next lane)`.
- **M11 closed:**
  `- [x] #task Old [fresh:: 2026-10-08] [completion:: 2026-10-08]` →
  tone `resting`, glyph `ring`, label `today`. Tooltip:
  `Confirmed today ⏎ Not in the review queue: Done`.
- **M12 unresolved, running:** as M2 with no resolution → tone
  `aging`, resolved false, tooltip as M2.
- **M13 unresolved, lease over:** `[fresh:: 2026-09-20]` on `[ ]`,
  unresolved → tone `aging`, label `18d`, remaining 0. Tooltip:
  `Confirmed Sun, Sep 20 · 18 days ago ⏎ Review lease ended Sun, Sep
  27 · every 7 days`.
- **M14 linked today:** a Ready `[fresh:: 2026-10-05]` task linked
  under today's open Pomodoro → tone `resting`. Line 2:
  `Not in the review queue: linked today`.
- **M15 other year:** `[fresh:: 2025-12-30]`, unresolved → label
  `282d`. Tooltip:
  `Confirmed Tue, Dec 30, 2025 · 282 days ago ⏎ Review lease ended
  Tue, Jan 6 · every 7 days`.
- **M16 quoted:**
  `> - [ ] #task Quoted [fresh:: 2026-10-05] [created::2026-09-01]` →
  a mark with foldSpace true.
- **M17 invalid refresh is not folded:**
  `[fresh:: 2026-10-05] [refresh:: 0]` → text
  `[fresh:: 2026-10-05]`, interval 7 (default), intervalLabel null;
  the refresh stays a pill.
- **M18 non-adjacent refresh:**
  `- [ ] #task X [refresh:: 14] [fresh:: 2026-10-05]` → text
  `[fresh:: 2026-10-05]`, interval 14 `(this task)`, intervalLabel
  null.
- **M19.** M9's line with `next_interval: false` gives tone `resting`,
  line 2 `Not in the review queue: Next`.
- **M20.** `- [/] #task Land it [fresh:: 2026-10-07] [refresh:: 30]`
  folds the refresh into the mark but shows no `/30d` suffix, because
  the effective source is `pending`. Tone `due`, label `1d`. Line 2:
  `Daily PENDING review due since Thu, Oct 8 · every 1 day (pending
  lane)`.
- **N1–N6, no mark (source null):**
  - N1 malformed `[fresh:: 2026-13-01]`;
  - N2 future `[fresh:: 2026-10-09]`;
  - N3 duplicate:
    `- [ ] #task A [fresh:: 2026-09-01] B [fresh:: 2026-09-20] [created::2026-09-01]`;
  - N4 misplaced:
    `- [ ] #task Buy milk [created::2026-09-29] [fresh:: 2026-10-01]`;
  - N5 parenthesized: `- [ ] #task Call mom (fresh:: 2026-10-05)`;
  - N6 not a task: `- Buy milk [fresh:: 2026-10-05]`.
- **C1–C3, consensus:**
  - C1: two `a.md` rows with source `[fresh:: 2026-10-01]`, both Ready
    and ROTTEN → the `due` model.
  - C2: the same pair, but one row is Next → null, so the mark is
    unresolved.
  - C3: no candidate rows → null.
- **MK1 no count:** `[fresh:: 2026-10-05]` with no `keeps` → `keeps 0`,
  dots null, tooltip as M2 with no keeps line.
- **MK2 aging with 2:** `[fresh:: 2026-10-05] [keeps:: 2]` (FRESH) →
  dots `••`, tone `aging`; without the card capability the keeps line
  is counting-only (`Kept 2 reviews in a row`).
- **MK3 due below threshold:** ROTTEN `[fresh:: 2026-09-30] [keeps:: 2]`
  → dots `••`, glyph `refresh` (not a leaf), line 3 `Alt+F to confirm`.
- **MK4 at-limit leaf:** ROTTEN `[fresh:: 2026-09-30] [keeps:: 3]` with
  `decide`, the nav card capability, and decay on →
  glyph `leaf`, `data-decide="true"`, dots `•••`, keeps line
  `Kept 3 reviews in a row · Bob asks at 3`, line 3 `Alt+F to decide`.
- **MK5 overflow:** `keeps 4` at limit 3 → dots `•••`, overflow `+1`
  (rendered `•••+1`), keeps line `Kept 4 reviews in a row · Bob asks at 3`.
- **MK6 repair:** duplicate `[keeps:: 2] [keeps:: 3]` or paren
  `(keeps:: 2)` never folds (`keeps` null); the fields stay visible
  Dataview pills with the dashed repair styling.

## 13. Rollout log

There is no freshness trial: no ritual change, release, or tuning waits on a trial window or keep rule; tune intervals or the budget whenever the walk needs it.

- 2026-10-01: per-note CROWDED surfaces (`bob ready`, the dash chip, crowded.md, the heading chips) went live; gesture notices are a separate follow-up.
- 2026-10-01: the tiered morning review walk went live (schema 3, lane intervals, tier notices, walk anchor).

## 14. Keep-streak rollout, rollback, and calibration

Deployed 2026-10-03: bob-navigation-hotkeys 1.69.0 plus bob-ledger-tools
1.24.0 synced byte-identical from the linked source, and `bob`
reinstalled from this checkout (schema 5, capture resets). Counting and
folded pips went live then; cards, the leaf, decision skip, and the
'next review asks' promise were still gated behind 2026-10-19 at that
release.

This release removes that calendar gate: bob-navigation-hotkeys 2.2.0
plus bob-ledger-tools 1.28.0, freshness JSON schema 8, freshness
namespace v6, and card capability v2. Decisions are available
immediately after installing compatible plugins. Mixed-version
sessions fall back: new nav + old ledger counts; new ledger +
old/missing nav shows pips without a card promise.

Rollback is config-only and never touches task lines: set
`freshness.decay: false` to disable card interception while preserving
counting and pips; the session mark toggle restores raw pills. This
rollout changed no global config, note intervals, or inbox residence.

Calibration is a lightweight human tally one month after this ungated
rollout: decision count, Keep/Drop outcomes, and RETURNED load for two
weeks after the first card wave, from counts, decision logs, and git
history — no telemetry service or automatic tuning. If most cards
(>50%) are Keep, consider limit 4; if most are Drop, consider limit 2.

Human smoke checklist (no Obsidian UI was available in this headless
rollout, so visual verification is still open; automated mark-surface
and modal interaction tests pass): light and dark themes, dense lines,
narrow widths, Live Preview and reading/Tasks/embeds views, dots and
leaf, raw reveal on selection and click, card focus order and key hints,
explicit cancellation, and one undo step — on fixture tasks only, never
by forcing live task dates or counters.
