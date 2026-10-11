# Daily Stats

Stats derives durable daily values from the vault and records only review-walk
answers, which freshness stamps cannot recover. bob-cli owns the metric meanings;
the terminal and Obsidian views consume the same JSON model.

This document defines the v1 review journal written by `bob-ledger-tools` and
`bob-navigation-hotkeys`. The journal is append-only and per host. It contains no
task text or derived metric values.

## Review journal

Each host appends to its own monthly file:

```text
.bob/stats/review/YYYY-MM.<host>.jsonl
```

The vault `.gitignore` must allow `!/.bob/stats/**/*.jsonl`, following the
`.bob/gkeep/imports/*.json` exception. Readers union the host files; they never
rewrite them. Duplicate events are harmless because `reviewed` counts distinct
row keys per day.

One line records one row answered by a committed review gesture:

```json
{"v":1,"ts":"2026-10-12T05:41:07-04:00","day":"2026-10-12","host":"apollo","kind":"review.answer","tier":"next","outcome":"keep","key":"sase.md#^fix-failed-agents","plugin":"bob-navigation-hotkeys@2.x"}
```

| Field | Meaning |
| --- | --- |
| `v` | Event schema version, currently `1`. |
| `ts` | Local timestamp with an explicit UTC offset. |
| `day` | Local calendar day of the review landing, `YYYY-MM-DD`. |
| `host` | Sanitized `os.hostname()` value. It also identifies the file owner. |
| `kind` | `review.answer`. |
| `tier` | The freshness evaluator's tier string, copied verbatim. |
| `outcome` | `keep`, `done`, `drop`, `defer`, `lane`, `route`, `today`, `sequence`, `card`, `checklist`, or `decay:<action>`. |
| `key` | The review walk's stable row key. |
| `plugin` | Writer identity and compatible major version. |

Readers accept both the shipped decay spellings (`decay:notNow`,
`decay:level:0`, `decay:lessOften`) and the normalized forms
(`decay:not_now`, `decay:level_0`, `decay:less_often`); any other
`decay:<action>` without whitespace also counts as recognized, so new decay
actions stay additive and journal files are never rewritten. Any other
unknown outcome lands under `other` in the per-day outcome parts.

The versioned `api.stats` namespace on bob-ledger-tools exposes
`append(event)`. It serializes appends through one queue, creates the journal
directory and file when needed, and rejects failed writes. Navigation calls it
only after a successful task write. A journal failure never fails the answer:
the event stays in an in-memory retry queue, and the plugin shows
“Review answer not recorded.”

## What gets recorded

The journal records an answer only when the gesture began on a validated `]s`
landing and its write committed:

- Resolving answers from `continueReviewWalkAfter`: completion, lane change,
  route, Today link, and resolving Task Card writes.
- Stay answers made through a successful freshness stamp.
- Successful decay-card decisions (`notNow`, `level:N`, `lessOften`, `reword`,
  `drop`, and `keep`).
- Completed PRE/POST checklist rows. They are recorded as `checklist`, but do
  not contribute to `reviewed` by default.

Navigation and skips, Ctrl+Shift+M moves, off-landing stamps, refusals, no-op
writes, unlink/reopen, automation, sync, and reloads do not create events.
Moving a row is not an answer; a later answer on that row is recorded.

`reviewed(D)` is the number of distinct row `key`s answered on day D in decision
tiers (all tiers except `pre` and `post`). `keep_rate` uses the last answer for
each row key per day among PENDING and NEXT. These definitions make duplicate
lines from a double press or two hosts harmless.

If the POST review checklist was closed on D but no host wrote a journal event
for D, `reviewed(D)` is unavailable (`null`) with a journal-gap warning. It is
never silently reported as zero. Each host's last event time provides a stale
writer signal.

## Conformance vectors

The machine-readable vectors live in
[`tests/fixtures/stats/review_answers.json`](../tests/fixtures/stats/review_answers.json).
They pin the writer's outcome mapping and which recorded events contribute to
`reviewed`; the stats reader uses the same fixture. Navigation tests mirror the
same cases. `reviewed` excludes PRE/POST checklist events while still retaining
them in the journal.

The selected Stats metric branch counts `#ref` reading tasks in Done, Created,
and Dropped as a separate `ref` kind part. Those derived task metrics are read
from vault data and are not journaled here.

## Engine principle

The engine derives every number at read time from vault data and never writes:
no caches, no stamps, no daily logging habit. The journal above is the only
data Stats ever records, because freshness stamps cannot recover review
answers. Reports are deterministic for a fixed clock (`BOB_NOW`): one vault
pass feeds all metrics, and repeated calls return byte-identical JSON.

## Metric kinds

Six configurable kinds share the common `label`, `unit`, `direction`, `color`,
`since`, and `description` fields. Palette values are color names
(`orange`, `violet`, `green`, `blue`, `gray`), never hex.

- `pomodoro`: closed `[x]`/`[X]` ledger rows under `## Pomodoros`.
  `measure: minutes` (default) uses each row's recorded `[t:: Nm]` first,
  then its time-range estimate (flagged `partial`); `measure: count` counts
  matching rows. `themes` / `exclude_themes` filter uppercase composite
  components (`GTD + SASE` splits its minutes across both parts, remainder to
  the first); unnamed rows are `(unnamed)`. Count mode attributes each row to
  its first matching component so parts add up to the total.
- `tasks`: `event: created` (any status on its created date), `done`
  (status type `DONE` with a completion date), or `dropped` (status type
  `CANCELLED` with a cancelled date). `recurring: exclude` (default), `only`,
  or `include`. `tags_any` / `tags_all` match whole case-insensitive tokens;
  `task_kind: task|prj|ref` filters the kind part (precedence `^prj`, then
  `#ref`/`^ref`, then task); `in:` matches note paths, basenames, or parent
  ancestry. The canonical daily note's `^gtd` row is always excluded.
- `checklist`: chores matched by all `tags` (exact case-insensitive tokens)
  plus an optional normalized `match` description substring, read from any
  indexed note (including archive rows mapped to their source residence).
  Recurring instances count. Chore identity strips inline fields, tags, and
  block IDs, collapses whitespace, and lowercases. An instance is outstanding
  on [`min(scheduled else created, completion)`, `completion`] so an early
  completion is met on its completion day. `measure: ratio` (default) reports
  met/due in basis points with pooled `num`/`den`, `count` reports chores
  met, `bool` reports whether every due chore was met. Nothing due is
  `null`/`missing`, never zero. A reopened row with a stale `[completion::]`
  is outstanding.
- `field`: one typed daily value from the day's canonical daily note.
  `field` names the key; `source` (default `either`) picks YAML frontmatter,
  `key:: value` body lines, or frontmatter-first with inline fallback only
  when the key is absent (a present but invalid value never falls back).
  Counts are non-negative integers, minutes accept `30m` / `1h 30m`, bools
  accept `0`/`1` / `true`/`false` / `yes`/`no`.
- `journal`: `select: reviewed` counts distinct row keys answered per day in
  the configured tiers; `select: keep_rate` reports the keep share (last
  answer per key, basis points) among `pending`/`next` rows. `tiers:
  decision` (every tier except `pre`/`post`), `all`, or an exact tier list.
  Duplicate lines and multiple hosts never inflate a day: the latest event
  per key wins (by instant, then path, then line).
- `formula`: integer arithmetic over metric IDs: literals, unary minus,
  parentheses, and `+ - * /` with checked math (half-away-from-zero
  division; division by zero and overflow are `null` with a reason, never a
  panic). Null inputs propagate null with the worst input quality.

## Built-in catalog

Built-ins are literal config entries validated exactly like custom metrics,
not a separate code path. `ref_in_done = yes`: `#ref`/`^ref` reading tasks
count in `done`, `created`, and `dropped` and appear as the `ref` kind part.

| ID | Kind and parameters | Unit / color / direction | Definition |
| --- | --- | --- | --- |
| `worked` | pomodoro, minutes | minutes / orange / up | Minutes in closed Pomodoros. Open Pomodoros are plans and never count. |
| `reviewed` | journal, reviewed, decision tiers | count / violet / up | Distinct walk row keys answered on day D in decision tiers. |
| `done` | tasks, done, recurring exclude | count / green / up | Non-recurring DONE tasks completed on D, minus the daily `^gtd` row. |
| `created` | tasks, created, recurring exclude | count / blue / neutral | Non-recurring tasks created on D, any status, same exclusions as Done. |
| `net` | formula, `created - done - dropped` | signed / gray / down | Daily flow balance; the delta reads shrinking/growing/flat by total sign. |
| `review_closed` | checklist, tags gtd+post, match Morning review, bool | bool / violet / up | The `#gtd #post` Morning review chore completed on D. |
| `dropped` | tasks, dropped, recurring exclude | count / gray / neutral | Non-recurring CANCELLED tasks cancelled on D, same exclusions as Done. |
| `habits` | checklist, tags gtd+pre, ratio | ratio / green / up | `#gtd #pre` chores met on D over chores due on D. |
| `keep_rate` | journal, keep_rate, pending+next | ratio / violet / neutral | Keep share of PENDING/NEXT answers on D. |
| `review_time` | pomodoro, minutes, themes GTD | minutes / orange / neutral | Worked minutes in GTD-themed sessions. |
| `sessions` | pomodoro, count | count / orange / neutral | Closed Pomodoro rows on D. |

## `stats:` config

```yaml
stats:
  range: 4w            # default window; 7d/4w/3m/1y, Nd/Nw/Nm/Ny, all, or
                      # inclusive YYYY-MM-DD..YYYY-MM-DD
  overview: [worked, reviewed, done, created, net, review_closed]
  hide: []            # suppresses lists and default selection, never evaluation
  since: 2026-06-01   # optional global floor for inferred tracking starts
  metrics:
    sase_time:        # pomodoro
      label: SASE time
      kind: pomodoro
      measure: minutes
      themes: [SASE]
    ref_done:         # tasks — note task_kind, not kind
      label: Ref done
      kind: tasks
      event: done
      task_kind: ref
    habit_count:      # checklist
      label: Habit count
      kind: checklist
      tags: [gtd, pre]
      measure: count
    energy:           # field
      label: Energy
      kind: field
      field: energy
      source: either
      unit: count
    all_reviewed:     # journal
      label: All reviewed
      kind: journal
      select: reviewed
      tiers: all
    done_and_dropped: # formula
      label: Done and dropped
      kind: formula
      expr: done + dropped
      unit: signed
```

**Defaults.** Missing config file, absent `stats:` block, and unknown keys
(including plugin-only `bob_path`) all fall back to defaults: range `4w`,
the six-metric overview, no hides, no global since. A custom entry whose ID
matches a built-in replaces the whole definition and is marked `redefined`;
an invalid replacement removes that metric with a warning instead of
silently falling back.

**Validation.** A malformed/non-mapping `stats` block or an invalid
top-level `range`, `overview`, `hide`, `since`, or `metrics` container is a
configuration error naming `stats.<key>` and the config path (exit 2 in the
future CLI); it never breaks the `plan:`, `freshness:`, or `highlights:`
loaders. A malformed metric entry is a local warning naming
`stats.metrics.<id>.<key>`; unrelated metrics keep working. Formula
references to unknown metrics and dependency cycles invalidate the affected
definitions (plus downstream dependents) with local warnings.

**Invalid values:**

- Unknown overview/hide names produce warnings, not errors.
- Unknown explicit metric IDs in a request are usage errors.
- Incompatible `unit` values for a kind drop that entry with a warning
  (`signed` is accepted for formulas, e.g. `net`; `unit: ratio` is rejected
  for formulas because they have no num/den). There is no unit fallback.
- Missing required fields (`label`, `kind`, task `event`, field `field`,
  formula `expr`, journal `select`) invalidate that metric entry only.

## Data model and quality

Every metric yields one value per date: an integer `value` (or `null`), an
optional ratio `num`/`den` in basis points, a quality, an optional non-ok
reason, and dimension parts (`theme`, `kind`, `note`, `tier`, `outcome`,
`met`/`missed`). Consumers never recalculate a display string, summary,
percentage, or delta — the report carries them all.

- `ok`: a real observed value (including a real zero, e.g. an empty
  Pomodoros section).
- `partial`: usable but estimated or incomplete (range-estimated minutes, a
  clipped bucket mixing known and missing days).
- `missing`: no value and no history claim (no daily note, no matching
  instance, a journal gap, an invalid field).
- `not_tracked`: before tracking began. Never a silent zero.

Quality precedence is `ok` < `partial` < `missing` < `not_tracked`: combining
inputs (formulas, grouped buckets) takes the worst.

**Since.** Dates before the effective start are `not_tracked`. An explicit
metric `since` wins; otherwise the start is inferred from the earliest
qualifying input (first `## Pomodoros` section, first valid task event date,
first checklist instance, first daily field, first journal event day); the
global `stats.since` is a floor above that. A metric with no input at all
never invents an ancient start. Formula starts are the latest dependency
start (every input must be tracked); literal-only formulas fall back to
explicit/global since, else the range start.

**Today is provisional.** The current day gets its own point with
`provisional: true`; summaries, prior windows, and grouped buckets never
include it. Day grouping still shows it as the trailing point.

**Journal gaps.** `review_closed` is evaluated as a dependency even when
hidden or unselected, so a known closed-review day with no journaled event
in any tier from any host reports `null`/`missing` plus a `journal_gap`
warning — never a silent zero. PRE/POST-only days carry real events, so
Reviewed reads a real zero there. Before the journal started, days stay
`not_tracked` rather than flagging plugin gaps. If `review_closed` itself is
invalid, journal metrics report a `journal_limitation` instead.

**Health.** Each report carries `health.tasks` (`invalid_dates`,
`closed_without_date`, and `missing_created` when nonzero — every affected
task counts once per limitation, never once per metric or day) and
`health.journal` (tracking start, malformed-line count, and per-host event
counts with the latest timestamp and day). `invalid_dates` counts a task
when created, done, or cancelled is malformed or future, or when scheduled
is malformed. A future `scheduled` date is ordinary planning data and does
not count. Daily-note field extraction skips task lines and ordered-list
task lines (`1. [ ] …`) so task fields never leak in as daily fields; quoted
lines are skipped the same way. YAML null frontmatter (`energy:`) is absent,
never the string `"null"`.

## Ranges, grouping, summaries, display

**Windows.** Presets yield N complete days ending yesterday plus the
separate provisional today point: `7d`, `4w`, every positive `Nd`/`Nw` /
`Nm` / `Ny`, and `all` (from the earliest effective start of the selected
metrics). Months convert to whole weeks with `ceil(N*52/12)` (`3m` = 91
days); years are `N*52` weeks (`1y` = 364). `offset: K` shifts the window
back `K*N` days and drops today. `prior` is the equal-length window right
before the current one and always resolves (it explains the delta);
`prior_points` are only filled on comparison requests. Explicit
`YYYY-MM-DD..YYYY-MM-DD` ranges clamp a future end to today and reject fully
future or inverted intervals with usage errors.

**Grouping.** `auto` picks day through 91 complete days, week through 371,
month beyond; weeks start Monday (ISO year boundaries included) and months
are calendar months. Edge-clipped buckets carry `partial: true`. Count,
minutes, and signed values sum; ratios pool `num`/`den` (periods never
average daily percentages); bools count met days as `N of M`, where M is
days with data. All-null buckets stay null. Week and month grouping keep
today as a separate final provisional point (date = end = today,
`rolling_median` null) whenever the window includes today; it is never
merged into a bucket.

**Summaries** always use raw complete daily values: the numeric headline is
the daily median (`/day` suffix), bool is met days over days with data
(`–` when that is 0 of 0), and ratios pool across days. Totals, means,
linearly interpolated `p25`/`p75` (index `(n-1)*p`, half-away rounding),
`days_with_data`, `days_met`, the prior headline, and a delta round it out.
Parts summaries rank dimensions by total; only `note` and `theme` collapse
to top five plus `other`. Rolling medians span 7 day points, 4 week buckets,
or 3 month buckets, with a 6-day / 3-week / 2-month lead-in so the first
displayed point can fill, need half the window in real observations, and
stay `null` for today.

**Display.** Minutes render `4h 05m` / `45m` / `0m`, counts plain, signed
values `−6` / `+9` / `0`, bools `met` / `missed`, ratios whole percentages,
null `–`. Deltas read `≈ same` within ±1% (and when a ratio difference
rounds to 0 pts), multipliers at ≥2× or ≤0.5×, `new` against a zero prior
(sign −1 when the current value is negative), `–` against a null prior,
`▼ 100%` for a drop to zero, percentage points for ratios, days for bools;
sign-crossing uses a raw amount. `net` instead reads shrinking/growing/flat
by total sign. Every delta names the prior period in words. Parts summary
values use minutes formatting only when the metric unit is minutes.

`all` with no history uses the same end convention as presets (end =
yesterday, zero days) and still includes today as provisional.

## Schema-1 contract

Reports serialize with `ok: true`, `schema_version: 1`, ISO dates, the range
(with prior metadata), per-metric definitions/summaries/points/parts/warnings,
health, and global warnings. Ratio points add `num`/`den`, grouped points
add `partial`, and empty maps (`parts`, `parts_summary`) are omitted.
`definition`, `limits`, `parameters`, and `total` are always present (total
may be JSON `null`). Optional fields that appear when present: `since`,
`reason`, `prior`, `delta`, `suffix`, `stat`, `journal.since`, and
`missing_created`.

```json
{
  "ok": true,
  "schema_version": 1,
  "today": "2026-10-11",
  "range": {
    "offset": 0, "start": "2026-10-08", "end": "2026-10-10",
    "days": 3, "includes_today": true, "group": "day",
    "label": "Oct 8 – Oct 10",
    "prior": {"start": "2026-10-05", "end": "2026-10-07", "label": "Oct 5 – Oct 7"}
  },
  "metrics": [
    {
      "id": "habits", "label": "Habits", "kind": "checklist",
      "unit": "ratio", "direction": "up", "color": "green",
      "source": "derived", "origin": "builtin", "since": "2026-10-09",
      "definition": "#gtd #pre checklist chores met on D divided by chores due on D.",
      "limits": ["Renaming a chore changes its identity."],
      "parameters": {"measure": "ratio", "tags": ["gtd", "pre"]},
      "summary": {
        "headline": {"value": 2500, "display": "25%", "stat": "pooled"},
        "total": null, "mean": 2500, "p25": 1250, "p75": 3750,
        "days_with_data": 2, "days_met": null
      },
      "points": [
        {"date": "2026-10-09", "end": "2026-10-09", "value": 0,
         "display": "0%", "quality": "ok", "provisional": false,
         "rolling_median": null, "num": 0, "den": 2,
         "parts": {"missed": {"journal": 1, "meditate": 1}}}
      ],
      "prior_points": [], "warnings": []
    }
  ],
  "health": {"journal": {"since": "2026-10-08", "malformed_lines": 1, "hosts": []},
             "tasks": {"invalid_dates": 1, "closed_without_date": 0, "missing_created": 1}},
  "warnings": []
}
```

**Errors** use a matching envelope (`ok: false`, `schema_version: 1`,
`error: {code, message}`). Vault I/O failures (unreadable vault) map to the
future CLI exit 1; configuration, request/unknown-metric, and non-Dataview
format failures map to exit 2. No `bob stats` command exists yet — these
codes reserve the contract the CLI phase will implement.

## Golden fixture and M-vector table

`tests/fixtures/stats/vault/` is a small hand-computed vault pinned to the
fixed clock Sunday 2026-10-11 noon and the explicit window
`2026-10-08..2026-10-11` (complete Oct 8–10, prior Oct 5–7, provisional
today Oct 11). Expected values below are derived by hand from the file
contents — never snapshotted from the implementation — and asserted field by
field in `src/native/stats/compute.rs`. Legend: plain numbers are `ok`;
`U` = `not_tracked`, `M` = `missing`, `G` = `missing` with `journal_gap`.

| # | Metric | Oct 8 | Oct 9 | Oct 10 | Today | What it proves |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | worked | U | 32 | 0 | 10·prov | Odd 7m composite splits 4/3; empty section is a real zero |
| M2 | reviewed | 0 | G | 2, last-wins | 1·prov | Pre-only day is a real zero; duplicate keys count once |
| M3 | done | U | 5, ref 1 | 4, prj 1 | 1·prov | `^gtd`, recurring, and reopened rows never count; archive maps to source |
| M4 | created | 2 | 5 | 5 | 1·prov | Archived creation surfaces on Oct 7; `^prj` and `[-]` add Oct 10 |
| M5 | net | U | U | −1 shrinking | 0·prov | Nulls stay null before the latest dependency start; 5−4−2 |
| M6 | review_closed | U | met | met | missed·prov | Today's POST TODO in `gtd_daily.md` is due and missed |
| M7 | dropped | U | U | 2 | 0·prov | `[!]` and `[-]` CANCELLED rows with cancelled dates |
| M8 | habits | U | 0/2 missed | 1/2 met | 0/2·prov | Skipped Journal, late Meditate, deferred Stretch; ratios pool 25% |
| M9 | keep_rate | M, no denom | G | 1/2 → 50% | 1/1·prov | Keep share with pooled basis points |
| M10 | review_time | U | 29 | 0 | 0·prov | GTD-only filter; SASE-only today scores zero, not missing |
| M11 | sessions | U | 2 | 0 | 1·prov | Count mode counts rows once; parts still attribute GTD |
| M12 | sase_time (custom pomodoro) | U | 3 | 0 | 10·prov | Custom theme metric through the same path |
| M13 | ref_done (custom tasks) | U | 1 | 0 | 0·prov | `task_kind: ref` spelling with the `ref` part |
| M14 | habit_count (custom checklist) | U | 0 | 1 | 0·prov | Count measure returns chores met |
| M15 | energy (custom field) | 3 frontmatter | 5 inline | M absent | M·prov | `either` source precedence and fallback |
| M16 | all_reviewed (custom journal) | 1, pre tier | G | 2 | 1·prov | `tiers: all` sees PRE/POST answers; gaps apply to customs too |
| M17 | done_and_dropped (custom formula) | U | U | +6 | +1·prov | Signed display; dependency-order evaluation |

Fixture edge coverage beyond the table: the malformed `2026-02-30` created
date counts once in `health.tasks` (`invalid_dates: 1`); future scheduled
dates are valid planning data; the undated open task counts once as
`missing_created`; the reopened open-status row with a completion date
leaves Done; `gtd_daily.md` plus `done/gtd_daily_done.md` drive checklist
chores from any note; a second journal host (`hermes`) carries an earlier
duplicate key and one malformed line; repeated `compute` calls return
byte-identical JSON without touching vault or config; an independent
`[t:: Nm]` text scan sums to the Oct 9 worked total; month grouping pools
one clipped October bucket plus a separate provisional today point; the
default request reports 4 weeks, six overview metrics, and 29 day points.

Writer vectors in `tests/fixtures/stats/review_answers.json` (R1–R19) pin
the plugin outcome mapping and which recorded events contribute to
`reviewed`; navigation tests mirror the same cases. Reader vectors in
`tests/fixtures/stats/journal_reader_vectors.json` pin last-answer order
(instant, then path, then line — including when string order and instant
order disagree), duplicate keys, two hosts, malformed and truncated lines,
unknown outcomes/tiers, pre/post-only days, keep-then-done, gap days, and
pre-tracking dates.

## Limitations

- Reopened tasks leave Done (only the current status type counts).
- Deleted tasks disappear from Created (history is read from live files).
- Keep imports retain their Keep date as `created`.
- Renaming a chore starts a new chore identity (history does not follow).
- Dates cannot reconstruct missing history: undatable or cancelled
  checklist instances report a `checklist_limitation`, never invented
  intervals.
- Grouped points cover complete days only; the provisional today point
  appears as a separate final point and never pools into a week/month bucket.
- Rolling medians need half a window of real observations, so short windows
  report `null` medians.
- The `^gtd` exclusion applies to the canonical daily note's own `^gtd` row
  (including archived rows mapped back to it).

## Later and rejected

Later (documented, not built): end-of-day lane sizes and walk-due-at-start
(needs a nightly `snapshot` kind recorded by `bob nightly`); day start
(first closed session's start); plan follow-through; manual metrics (energy,
sleep, exercise — already supported by the `field` kind, none ship enabled);
an approximate review backfill from vault git history (the journal starts
clean and shows "Tracking since …").

Rejected: stamp counts, productivity scores, tasks per hour, automated
"insights", streak counters, commits/words/lines, time in Obsidian.

## Performance

One vault pass feeds every metric; only `.bob/stats/review/*.jsonl` is read
besides Markdown. Measured in release mode on the live vault (BOB_NOW pinned
to 2026-10-11): this landing-fixes run recorded 573 ms for the full read
plus a 7-day report (the land agent recorded 365–378 ms at f9324da). The
≤300 ms target is rollout/CLI acceptance
(bob-cli-6l.6 / bob-cli-6l.3). An opt-in parity test
(`BOB_STATS_REAL_VAULT_PARITY=1` with `BOB_NOW`, vault from `BOB_DIR` or
`~/bob`) re-checks sessions, worked, done, created, and dropped against
independent line-regex oracles on every live run.
