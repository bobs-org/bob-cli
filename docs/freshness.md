# Task freshness

Every visible, non-recurring Ready task carries a human-confirmed
`[fresh:: YYYY-MM-DD]`: the local calendar date a human last confirmed
that the task still needs doing as written — its wording, priority,
project, schedule, and dependencies. A task that was never confirmed,
or was confirmed longer ago than its refresh interval, is due for
review. The morning review then costs roughly "pool ÷ interval +
arrivals" glances instead of the whole pool.

This file is the contract both implementations cite. The Rust side is
`src/native/freshness/` (`placement.rs`, `state.rs`) with the
`freshness:` config block in `src/native/config/freshness.rs`; the
JavaScript mirror is `api.freshness` in bob-ledger-tools (api v3). The
bob-ledger-tools JavaScript tests use the conformance vectors below
verbatim.

## 1. Definition

- **Task freshness** (aka **freshness**) is the local calendar date a
  human last confirmed that an open task still needs doing as written.
  It is stored as `[fresh:: YYYY-MM-DD]`.
- **Stamp / refresh:** write today's date into `fresh`.
- **Due for review:** an in-scope task in state NEW, RESURFACED, or
  STALE (see §4).
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
| `freshness.stale_daily_budget` | `~/.config/bob/config.yml`                     | optional integer ≥ 1, default off        |

**Interval precedence.** `interval(t)` is the task's `refresh`, then
the containing note's `task_refresh`, then `freshness.interval`, then
7. The note override applies by residence (the note that contains the
task), not through `parent` links.

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
  interval: 7 # days before a confirmed Ready task is due for review (docs/freshness.md)
  # stale_daily_budget: 15 # optional daily goal meter; never hides tasks
```

## 3. Placement rule

The Rust helper is `stamp_fresh` / `set_refresh`; the JavaScript one
is `api.freshness.stampLine` / `setRefreshLine` in bob-ledger-tools.
One helper per language, pinned by the shared vectors below.

1. **Scope.** The helpers handle Tasks' Dataview format only, which is
   the vault's format. `bob freshness` refuses to run when the vault's
   Tasks settings use another format.
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
              | STALE       if today ≥ fresh(t) + interval(t)   (stamped Mon at 7 ⇒ due next Mon)
              | FRESH       otherwise
due_on(t)     = RESURFACED: scheduled(t); STALE/FRESH: fresh(t) + interval(t); NEW: none
due(t)        = in_scope(t) ∧ state(t) ≠ FRESH
tier(t)       = NEW | DUE (RESURFACED and STALE together)
queue order   = NEW by (path, line); then DUE by (due_on, path, line)
counts        = due, new, resurfaced, stale, fresh (in-scope FRESH),
                refreshed_today (tasks of any status, outside _templates/_conflicts,
                whose fresh(t) == today), budget, budget_met
                (budget set ∧ refreshed_today ≥ budget ∧ new == 0)
```

RESURFACED beats STALE when both hold: a deferral that returned is
due as soon as it returns. The tickler makes a short deferral (for
example a P1 roll of 2–7 days) due as soon as it returns, without any
hooks write.

Line numbers are 1-based in JSON and docs. Tasks' `lineNumber` is
0-based, so convert it.

**Lints:** `fresh_malformed`, `fresh_future`, `fresh_duplicate`,
`fresh_misplaced` (a `fresh`/`refresh` inside the Tasks suffix; the
next stamp repairs it), `refresh_invalid` (per task) and
`task_refresh_invalid` (per note). `today_link_unresolved` passes
through unchanged from the Today engine.

## 5. Who stamps

**Rule:** a supported human gesture that already rewrites an open
task's line stamps that task in the same write, as the last
transformation of the line, if the task is still open and
non-recurring afterwards. Stamps apply in every lane, because a Next,
Pending, or Blocked task may come back to Ready later. A stamp dated
today on a canonical line is a no-op.

| Surface                | Stamps | Never stamps |
| ---------------------- | ------ | ------------ |
| bob-navigation-hotkeys | Alt+F and Alt+Shift+F (their only change); Alt+N commit and release; the Ctrl+Shift+P priority, scheduled, dependsOn, delete-property (Ctrl+D), lane and new refresh rows in single, counted and Task Link mode; each open task moved by Ctrl+Shift+M; the `!` dependency toggle when it rewrites the parent task line | the cancel row, project-frontmatter edits, create-project-note-from-task |
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
doesn't stamp.

## 6. Review ritual

**Morning (≈10 min; replaces reading READY):**

1. Run `bob gkeep pull`.
2. Walk `]s` / Alt+Shift+F until the status bar shows **0 new**. This
   step is never capped and never skipped.
3. Continue through DUE until 0 due, or until the budget meter is met.
4. Then PENDING → NEXT: link today's work and release the rest.

**Weekly:** one more line in the existing Weekly prune chore. Clear
any leftover DUE, or lengthen that note's `task_refresh`, and look for
projects with no Next or Ready task.

Review outcomes, one key each (every row except "edit" stamps by
itself): still right (Alt+Shift+F or Alt+F); see it less often
(Ctrl+Shift+P → refresh → 14 / 30 / 90); not now (Ctrl+Shift+P
priority rolls a P-level `scheduled`); do today (Ctrl+Shift+Enter /
Alt+N); route to a project (Ctrl+Shift+M); drop (Ctrl+Shift+P
cancel); wording wrong (edit, then Alt+F).

## 7. `bob freshness`

Headless review queue (`bob freshness list`, human and JSON) and a
guarded, idempotent, staggered cutover seed (`bob freshness seed`)
that aborts on any parse change. Landed by `fresh-cli`; this section
is a placeholder until then.

## 8. Surfaces

| Surface | Phase |
| ------- | ----- |
| `bob freshness` | fresh-cli |
| `bob capture` | capture-stamps |
| bob-ledger-tools | ledger-freshness |
| bob-navigation-hotkeys | nav-review, nav-stamps |
| task-status-cycler | cycler-link-stamps |
| block-id-prompt | cycler-link-stamps |
| `freshness.md` | vault-review |
| `dash.md` | vault-review |

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

## 10. State conformance examples

Today `2026-10-08`, config interval 7, and a Ready, visible,
non-recurring task in `a.md` unless noted. The bob-ledger-tools
JavaScript tests use these verbatim.

- **S1 new:** no `fresh` → `new`
- **S2 fresh:** `fresh 2026-10-02` → `fresh`, `due_on 2026-10-09`
- **S3 boundary:** `fresh 2026-10-01` → `stale`, `due_on 2026-10-08`,
  `days_overdue 0`
- **S4 overdue:** `fresh 2026-09-20` → `stale`, `due_on 2026-09-27`,
  `days_overdue 11`
- **S5 task beats note:** `[refresh:: 14]`, note `task_refresh: 3`,
  `fresh 2026-10-01` → `fresh` (task source, due 2026-10-15)
- **S6 note beats config:** note `task_refresh: 3`, `fresh 2026-10-05`
  → `stale` (note source)
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
  open Pomodoro; a `[ ]` whose `dependsOn` names an open task
- **S14 queue order.** Given NEW `b.md:3` and NEW `a.md:9`; STALE due
  2026-10-01 at `c.md:2`; RESURFACED due 2026-10-07 at `a.md:4`; STALE
  due 2026-10-07 at `a.md:2`. The order is `a.md:9`, `b.md:3`,
  `c.md:2`, `a.md:2`, `a.md:4`.
- **S15 counts:** `refreshed_today` counts a `[*]` and an `[x]`
  stamped 2026-10-08, but not a Ready task stamped 2026-10-07. With
  budget 15, 15 refreshed and 0 new → `budget_met: true`; with 1 new
  → `false`.
