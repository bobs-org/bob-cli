# Task freshness

Every visible, non-recurring Ready task carries a human-confirmed
`[fresh:: YYYY-MM-DD]`: the local calendar date a human last confirmed
that the task still needs doing as written — its wording, priority,
project, schedule, and dependencies. A task that was never confirmed,
or was confirmed longer ago than its refresh interval, is due for
review. The morning review then costs roughly "pool ÷ interval +
arrivals" glances instead of the whole pool.

The daily lane review reuses the same `[fresh::]` stamp for Pending
(`[/]`) and Next (`[*]`) tasks: each lane has a review cadence
(`freshness.pending_interval` / `freshness.next_interval`, default 1
day), and the morning walk visits NEW → PENDING → NEXT → RETURNED →
ROTTEN in explicit tiers.

This file is the contract both implementations cite. The Rust side is
`src/native/freshness/` (`placement.rs`, `state.rs`) with the
`freshness:` config block in `src/native/config/freshness.rs`; the
JavaScript mirror is `api.freshness` in bob-ledger-tools (top-level
api v3, freshness namespace v4). The bob-ledger-tools JavaScript
tests use the conformance vectors below verbatim.

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
| `task_refresh: N`              | frontmatter of the note containing the task    | optional integer days, 1–365             |
| `freshness.interval`           | `~/.config/bob/config.yml`                     | integer days, 1–365, default 7           |
| `freshness.pending_interval`   | `~/.config/bob/config.yml`                     | integer days, 1–365, or `false`; default 1 |
| `freshness.next_interval`      | `~/.config/bob/config.yml`                     | integer days, 1–365, or `false`; default 1 |
| `freshness.rotten_daily_budget` | `~/.config/bob/config.yml`                    | optional integer ≥ 1, default off        |

**Interval precedence.** `interval(t)` for a lane task in a walked
lane is that lane's interval (`pending_interval` for `[/]` with source
`pending`, `next_interval` for `[*]` with source `next`), overriding
the whole Ready chain below. Otherwise `interval(t)` is the task's
`refresh`, then the containing note's `task_refresh`, then
`freshness.interval`, then 7. The note override applies by residence
(the note that contains the task), not through `parent` links.
`pending_interval: false` / `next_interval: false` turns that lane's
walk off; an absent or null value means the default 1, matching how
`interval:` already treats null.

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
  # rotten_daily_budget: 15 # counts upkeep outside the lanes; never hides tasks
```

**Mobile config caveat.** On mobile, when the config file is
unavailable, ledger-tools uses the defaults above and marks them
`invalid` rather than failing; a config edit on desktop is visible
within the existing 60-second tick. `bob freshness` on desktop still
exits 2 for an invalid `freshness:` block.

## 3. Placement rule

The Rust helper is `stamp_fresh` / `set_refresh`; the JavaScript one
is `api.freshness.stampLine` / `setRefreshLine` in bob-ledger-tools.
One helper per language, pinned by the shared vectors below.

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
     - a `fresh` or `refresh` field;
     - a trailing tag (the `HASH_TAG_AT_END` grammar).
   - Never scan past the start of the task body. If the body begins
     with the global filter token `#task`, never scan past the end of
     that token.

   The **Tasks suffix** starts at the leftmost Tasks element of that
   run (a Tasks-key field, a tag, or `^id`). `fresh` / `refresh`
   fields at the run's left edge are not part of it.
3. **Canonical output.**
   - Remove every `fresh` and `refresh` field on the line. Each
     removal also collapses the whitespace it leaves to a single
     space.
   - Rebuild as `head.trim_end() + " " + "[fresh:: D]" + ("
     [refresh:: N]" if a refresh value is kept) + (" " + suffix if
     there is a suffix)`.
   - The kept refresh value is the first valid existing one;
     `set_refresh` replaces or removes it.
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
                  not dependency-blocked, no #hide, not under _templates or _conflicts,
                  no scheduled date after today
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
                (lane-visible is the existing NEXT/PENDING predicate, unchanged)
lane_interval = freshness.pending_interval (pending) | freshness.next_interval (next);
                default 1; false = that lane is not walked
interval(t)   = lane task with a walked lane: lane_interval, source "pending" | "next"
                otherwise unchanged: task refresh → note task_refresh → freshness.interval → 7
lane_due(t)   = walked lane ∧ (no fresh(t) ∨ today ≥ fresh(t) + lane_interval)
due_on(t)     = lane row: fresh(t) + lane_interval, or none when never stamped
                RESURFACED: scheduled(t); ROTTEN/FRESH: fresh(t) + interval(t); NEW: none
due(t)        = in_scope(t) ∧ state(t) ≠ FRESH
tier(t)       = new       if lane ready ∧ state NEW
              | pending   if lane pending ∧ walk_scope ∧ lane_due
              | next      if lane next ∧ walk_scope ∧ lane_due
              | returned  if state RESURFACED
              | rotten    if state ROTTEN
              | none      otherwise
```

The queue holds every row with a tier, in tier order. Within each
tier the order is:

| Tier     | Order within the tier                                    |
| -------- | -------------------------------------------------------- |
| new      | path ↑, line ↑ (unchanged)                               |
| pending  | never-stamped first, due_on ↑, created ↑, path ↑, line ↑ |
| next     | same as pending                                          |
| returned | due_on (= scheduled) ↑, created ↓, path ↑, line ↑        |
| rotten   | interval ↑, due_on ↑, created ↓, path ↑, line ↑          |

A missing `created` always sorts after dated peers within its tier,
in both ascending and descending keys. The commitment tiers are new,
pending, next, and returned. Rotten is upkeep.

Counts:

- `due`, `new`, `resurfaced`, `rotten`, `fresh`, and
  `refreshed_today` keep their meaning. `due` stays Ready-only.
- New counts: `pending_due`, `next_due`, and `walk` (the full queue
  length, before any `--limit`).
- New count `upkeep_today`: tasks of any status outside `_templates`
  / `_conflicts` whose `fresh` equals today and whose status symbol is
  neither `/` nor `*`.
- `budget_met = budget set ∧ upkeep_today ≥ budget ∧ new == 0`.

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

**Machine vocabulary (schema 2).** Human output, help, and docs
say `rotten`, and so does the machine contract since the vocab-rotten
migration published JSON schema 2: `state: "rotten"`,
`counts.rotten`, and `freshness.rotten_daily_budget`. Each JSON queue
row still carries the `bucket` field (`"new"`, `"rotten"`, or null).
Likewise bob-ledger-tools uses the `"rotten"` state string under
freshness namespace v3 (`api.freshness.version === 3`; top-level api
stays v3).

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
`fresh_misplaced` (a `fresh`/`refresh` inside the Tasks suffix; the
next stamp repairs it), `refresh_invalid` (per task) and
`task_refresh_invalid` (per note),
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
| bob-navigation-hotkeys | Alt+F and Alt+Shift+F (their only change); Alt+N commit and release; the Ctrl+Shift+P priority, scheduled, dependsOn, delete-property (Ctrl+D), lane and new refresh rows in single, counted and Task Link mode; each open task moved by Ctrl+Shift+M; the `!` transclusion toggle when it rewrites the parent task line; the Ctrl+Enter recommended roll and decay | the cancel row (including the decay cancel), project-frontmatter edits, create-project-note-from-task |
| task-status-cycler     | Alt+[ / Alt+] (including counted and transcluded targets) when the result is an open status, including leaving Blocked by hand; Ctrl+Enter reopening a done task | closing (done or cancelled), Ctrl+Shift+] bullet → `#task` (that is creation), the dependency-ID normalizer, `recoverBlockedDependents` |
| block-id-prompt        | Ctrl+Shift+Enter and `^^` when they rewrite the task line (Ready/Blocked → Next, a new block ID) | unlink, Task Link removal, Ctrl+6 rename |
| `bob capture`          | `plan_task_link` (the link direction of `@route+id!`, Ensure Next, solo `@route:id` / `^route:id`, link-then-close) and the `=x` rows that set `[/]` | new tasks on any route, `=x` complete, unlink, start rows, sub-bullets |
| Automation             | — | hooks, `projects sync`, `randomize`, `gkeep pull`, `highlights`, `move-done-tasks`, `nightly`, `vault-sync`, `capture-task-id` |
| `bob freshness seed`   | the one-time cutover (a documented exception) | — |
| Hand editing           | — | not monitored; edit, then press Alt+F |

Deferrals: scheduling a task into the future stamps it, and it becomes
Blocked. When it returns it is RESURFACED. Automatic Blocked → Ready
returns through the hooks never stamp.

Placement is the exception to the copy-small-helpers rule: nav,
task-status-cycler, and block-id-prompt call
`api?.freshness?.stampLine?.(line, dateText) ?? line` on
bob-ledger-tools (api `version >= 3`), with a source comment at each
call site. A missing stamp only means Bryan sees the task once more;
a misplaced stamp hides Tasks fields — so the risky part lives in one
place, and when ledger-tools is absent or old the gesture simply
doesn't stamp. Clicking a freshness mark only reveals the raw field
for editing and never stamps.

## 6. Review ritual

**Morning, about 10 minutes once the lanes are at their caps:**

1. Run `bob gkeep pull`.
2. Use `]s` / Alt+Shift+F through NEW → PENDING → NEXT → RETURNED
   until the notice says **Commitments done**. NEW is never capped or
   skipped. In the lanes, ask "still in this lane?": keep with
   Alt+Shift+F, do it today with Ctrl+Shift+Enter, release with Alt+N.
   For a returned deferral, "not now" is a priority roll, not Alt+F.
3. Start the highlight.
4. Clear CROWDED to 0 (split, sequence, defer, drop via `bob ready`).
   This does not depend on how far ROTTEN review got.
5. Then, or later, do ROTTEN upkeep until 0 or the budget. It is fine
   to stop partway.

**First walk:** release the lanes to their caps.

**Weekly:** if more than about 90% of NEXT reviews end in keep,
lengthen `next_interval` to 2–3 and leave Pending at 1.

Review outcomes, one key each (every row except "edit" stamps by
itself): still right (Alt+Shift+F or Alt+F); see it less often
(Ctrl+Shift+P → refresh → 14 / 30 / 90); not now (Ctrl+Shift+P
priority rolls a P-level `scheduled`); do today (Ctrl+Shift+Enter /
Alt+N); route to a project (Ctrl+Shift+M); drop (Ctrl+Shift+P
cancel); wording wrong (edit, then Alt+F).

## 7. `bob freshness`

Headless review queue (`bob freshness list`, human and JSON) and a
guarded, idempotent, staggered cutover seed (`bob freshness seed`)
that aborts on any parse change.

Running `bob freshness` with no subcommand runs `list`.

`list` options: `-f/--format human|json` (default `human`) and
`-l/--limit N` (queue rows only; counts always cover the whole
vault). Human output is colored only on a TTY:

```text
bob freshness · Thu 2026-10-08 · every 7d · pending 1d · next 1d

  REVIEW 66 due · 1 new · 10 pending · 15 next · 14 returned · 26 rotten · ✓ 12 today

  NEW 1
    gkeep_inbox.md:14   Pick up our daughter    created 2026-09-30
  PENDING 10
    sase.md:40          Land the epic           never confirmed · created 2026-09-20
    work.md:12          Ship the report         due today · fresh 2026-10-07 · every 1d (pending)
  NEXT 15
    …
  RETURNED 14
    b.md:40             Week habits             returned · scheduled 2026-10-07 · fresh 2026-10-05
  ── commitments done above · upkeep below ──
  ROTTEN 26
    d.md:1              Water the herbs         due today · fresh 2026-10-07 · every 1d (task)
    a.md:2              Rename queue input      rotten 3d · fresh 2026-09-28 · every 7d (note)
```

- The `REVIEW N due` total is `walk`.
- Human vocabulary says "returned"; the machine `state` stays
  `resurfaced`.
- A disabled lane shows `pending off` in the header.
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

The JSON contract is `schema_version: 3` with `ok`, `date`,
`config` (`interval`, `pending_interval` / `next_interval` as a number
or `false`, `rotten_daily_budget`), `counts` (`due`, `new`,
`resurfaced`, `rotten`, `fresh`, `pending_due`, `next_due`, `walk`,
`refreshed_today`, `upkeep_today`, `budget`, `budget_met`), `queue`
(each with `rank`, `tier` (`new` | `pending` | `next` | `returned` |
`rotten`), `lane` (`ready` | `pending` | `next`), `state`, `bucket`
(`"new"`, `"rotten"`, or null — lane rows carry `state: null` and
`bucket: null`), `path`, `line`, `block_id`, `status_symbol`, `text`,
`created`, `fresh`, `interval`, `interval_source`, `due_on`,
`days_overdue`), and `warnings` (`code`, `path`, `line`, `message`).

`seed` options: `-d/--dry-run`, `-F/--force`, `-f/--format
human|json`. Ready tasks without a valid `fresh` are grouped by note
and bin-packed largest-note-first into 7 buckets; a note bigger than
`ceil(total / 7)` splits into consecutive line-order chunks, and ties
break by path, then bucket index. Bucket `k` (1–7) lands on `today −
7 + k`, raised per task to `max(bucket date, today − interval(t) +
1, scheduled(t) when due)` so nothing is due on cutover day and
nothing arrives RESURFACED, and clamped to today. Every other open,
non-recurring task outside `#hide`, `_templates`, `_conflicts`, and
daily notes gets today. The seed refuses when any such task already
carries a `fresh` dated before today (unless `--force`), aborts the
whole run with no writes when any changed line parses differently
under either Rust parser, re-reads each file just before writing and
refuses when one changed, and writes through a temp file plus rename.
A same-day rerun finds nothing to stamp and reports zeros. The JSON
contract is `schema_version: 2` with `ok`, `date`, `dry_run`,
`stamped` (`ready`, `other`), `buckets` (`fresh`, `due_on`, `count`,
`notes`), `skipped` (`already_stamped`, `recurring`,
`out_of_scope`), `files`, and `warnings`. The shared constant also
moves the `seed` envelope to 3, with seed content unchanged.

Exit codes: 0 on success; 1 for I/O errors and seed refusals; 2 for
an invalid `freshness:` block or a non-Dataview task format.

## 8. Surfaces

| Surface | Phase |
| ------- | ----- |
| `bob freshness` | fresh-cli (landed: `list` and `seed` in `src/native/freshness/`); tiered walk (schema 3: `list` walks NEW → PENDING → NEXT → RETURNED → ROTTEN with lane intervals) |
| `bob capture` | capture-stamps (landed: plan_task_link + `=x` close stamp via `stamp_fresh`) |
| bob-ledger-tools | ledger-freshness (landed: api v3 `api.freshness` + status bar in 1.8.0) |
| bob-navigation-hotkeys | nav-review, nav-stamps (landed: Alt+N + Ctrl+Shift+P/Ctrl+Shift+M/! stamping + refresh row in 1.44.0) |
| task-status-cycler | cycler-link-stamps (landed: Alt+[/Alt+] + Ctrl+Enter reopen stamping in 1.18.0) |
| block-id-prompt | cycler-link-stamps (landed: Ctrl+Shift+Enter + ^^ stamping in 1.16.0) |
| `rotten.md` (aliases `Review`, `Freshness review`, `Rotten Tasks`) | dash-gating (landed: live summary plus always-present RETURNED and ROTTEN groups; tasks stay in source notes, rows are click-through views) |
| `dash.md` | dash-gating (landed: TODAY → NEW → PENDING → NEXT → READY sections; NEW/PENDING/NEXT/READY/CROWDED/BLOCKED/ROTTEN/TODAY chips; gated READY with whole-lane tooltip; CROWDED via `noteReady` v1 opening crowded.md) |
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
` (pending lane)` / ` (next lane)` for the lane sources. Line 1:
`Confirmed today` at age 0, otherwise `Confirmed {date} ·
{relative}`. Line 2 is picked by resolution rather than tone: closed
or out of scope → `Not in the review queue: {reason}`; a due lane
row → `Daily {PENDING|NEXT} review due since {dueOn} · every …`; a
lane row stamped today → `Next review {fresh+interval} · every …
({pending|next} lane)`; ROTTEN → `Due for review since {dueOn} ·
every …`; RESURFACED → `Resurfaced {scheduled}: scheduled after it
was confirmed`; FRESH or unresolved with the lease running → `Next
review {fresh+interval} · every …`; unresolved with the lease over →
`Review lease ended {fresh+interval} · every …`. Line 3, `due` tone
only: `Alt+F to confirm` (lane rows add the lane keep/release/today
keys per M9). Lane rows with a tier get the `due` tone and lane rows
stamped today get the `today` tone. Lane tasks outside the walk
(Today, daily note, disabled lane, recurring) keep `resting` with the
existing reasons. Reasons by status symbol: `*` Next, `/` In Progress, `?`
Blocked, `x`/`X` Done, `-` Cancelled, any other non-space symbol
`status [s]`; for `[ ]`, the first that applies: `linked today`, `in
a daily note`, `recurring`, `in _templates or _conflicts`,
`scheduled for {date}`, otherwise `hidden or dependency-blocked`.

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

**Live verification (Bryan, in Obsidian).**

- Task lines in Live Preview show `✓ today`, the ring with `Nd`,
  and `⟳ Nd` in an orange capsule; no Dataview `FRESH` pill appears
  beside a mark.
- Moving the cursor into a mark, or clicking it, reveals
  `[fresh:: …]` for editing.
- Alt+F on a `⟳` task flips it to `✓ today` within about a second.
- Ctrl+Shift+P → refresh 14 shows `/14d` from the next day.
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

## 13. Two-week trial (2026-10-05 through 2026-10-18)

The accepted trial runs 2026-10-05 through 2026-10-18 (if rollout
misses the start, record the actual dates for a full 14-day trial).
The walk changes the ritual the trial measures. If the walk has not
shipped by Mon 2026-10-05, record that the 14-day trial starts the day
it lands. The rollout phase records the actual dates. Don't change the
ritual mid-trial. Completion means the trial is ready to run, not that
an agent waits two weeks or claims its outcome.

- 2026-10-01: per-note CROWDED surfaces (`bob ready`, the dash chip,
  crowded.md, the heading chips) went live; gesture notices are
  deferred until after the trial.
- 2026-10-01: the tiered morning review walk went live (schema 3, lane
  intervals, tier notices, walk anchor). The trial window
  2026-10-05 through 2026-10-18 stands.

Keep a lightweight daily tally on the rotten page: NEW, RETURNED,
expired ROTTEN, confirmed FRESH, READY, lanes kept/released, minutes to
Commitments done, and whether the chip was red.
Track confirmed FRESH separately from exempt READY for the rule below.

Keep if red on no more than 3 mornings, at least about 30 confirmed
tasks on most mornings, and no lost-needed-task case. If it fails,
Bryan can first adjust intervals or budget, then reconsider
dim-not-hide; never adopt a stored rotten tag.
