# Daily Stats

Stats derives durable daily values from the vault and records only review-walk
answers, which freshness stamps cannot recover. bob-cli owns the metric meanings;
the terminal and Obsidian views consume the same JSON model.

`bob stats` is the terminal glance. The Obsidian Stats page
(`dash_stats.md`, STATS chip on the dashboard) is the same report drawn
by the `bob-stats` block; see [dashboard.md](dashboard.md#child-pages).
Journal files under `.bob/stats/review/` travel with the vault; see
[vault-git-sync.md](vault-git-sync.md#stats-review-journal). The
[`bob stats` CLI reference](reference/cli/stats.md) indexes the command.

This document defines the v1 review journal written by `bob-ledger-tools` and
`bob-navigation-hotkeys`. The journal is append-only and per host. It contains no
task text or derived metric values.

## Review journal

Each host appends to its own monthly file:

```text
.bob/stats/review/YYYY-MM.<host>.jsonl
```

The vault `.gitignore` must allow `!/.bob/stats/**/*.jsonl`, following the
`.bob/gkeep/imports/*.json` exception. That allowance is part of the Git
sync runbook
([vault-git-sync.md](vault-git-sync.md#stats-review-journal)). Readers
union the host files; the vault `.gitattributes` also union-merges the journals so
concurrent appends from different hosts survive a Git merge. Readers never rewrite
them. Duplicate events are
harmless because `reviewed` counts distinct row keys per day.

One line records one row answered by a committed review gesture:

```json
{"v":1,"ts":"2026-10-12T05:41:07-04:00","day":"2026-10-12","host":"apollo","kind":"review.answer","tier":"next","outcome":"keep","key":"sase.md#^fix-failed-agents","plugin":"bob-navigation-hotkeys@2.19.0"}
```

| Field | Meaning |
| --- | --- |
| `v` | Event schema version, currently `1`. |
| `ts` | Local timestamp with an explicit UTC offset. |
| `day` | Local calendar day of the review landing, `YYYY-MM-DD`. |
| `host` | Sanitized `os.hostname()`: lowercase, cut at the first `.`, non-`[a-z0-9-]` → `-`, trimmed; fallback `host`. `Kellys-MBP` becomes `kellys-mbp`, matching the `vault(kellys-mbp)` label. |
| `kind` | `review.answer`. |
| `tier` | The freshness evaluator's tier string, copied verbatim. |
| `outcome` | `keep`, `done`, `drop`, `defer`, `lane`, `route`, `today`, `sequence`, `card`, `checklist`, or `decay:<action>`. A Task Card answer maps from the after-line: `[x]`/`[X]` → `done`, `[-]` → `drop`, scheduled date moved past today → `defer`, a new `dependsOn` id → `sequence`, otherwise `card`. |
| `key` | The review walk's stable row key. |
| `plugin` | `<writer>@<manifest version>`, for example `bob-navigation-hotkeys@2.19.0`. Earlier lines may carry `@2.x`. |

Readers accept both the shipped decay spellings (`decay:notNow`,
`decay:level:0`, `decay:lessOften`) and the normalized forms
(`decay:not_now`, `decay:level_0`, `decay:less_often`); any other
`decay:<action>` without whitespace also counts as recognized, so new decay
actions stay additive and journal files are never rewritten. Any other
unknown outcome lands under `other` in the per-day outcome parts.

The versioned `api.stats` namespace on bob-ledger-tools exposes
`append(event)` and `host()`. `host()` returns the sanitized hostname used
for both the file name and the event `host` field. `append` serializes through
one queue, creates the journal directory and file when needed, and rejects
failed writes (validation errors carry `code: "invalid_review_event"`).
Navigation calls it only after a successful task write. A journal failure
never fails the answer: the event stays in an in-memory retry queue (cap 500;
oldest dropped with a counted warning). Missing or old `api.stats` shows one
Notice per session, “Update Bob Ledger Tools to record review stats”, and
keeps retrying quietly. A write-failure burst shows one Notice, “Review
answer not recorded — will retry”; a later successful drain shows “Review
journal recovered: N answers written”. Validation rejects are dropped with a
console warning instead of retrying forever.

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
[`tests/fixtures/stats/review_answers.json`](https://github.com/bobs-org/bob-cli/blob/master/tests/fixtures/stats/review_answers.json).
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
  its first matching component so parts add up to the total. `in:` /
  `exclude_in:` match a linked task's residence chain with the same
  path, basename, and parent-ancestry rules as task `in:`. Minutes mode
  keeps those tasks' even shares and re-splits theme parts only when a filter
  removes a task share; count mode keeps the session when any task matches and
  picks the lead home among kept tasks. Whole review and unlinked sessions never
  match `in:` and are never removed by `exclude_in`. `review: include`
  (default), `only`, or `exclude` selects review-bucket shares: whole named or
  bridged review sessions are all review, while a work session's `^gtd` task
  share is review and its other task shares are not. `only` keeps those review
  shares; `exclude` drops them. Count mode counts a session once if any share
  remains and chooses the lead home among the kept tasks. Home and review
  filters AND with `themes` / `exclude_themes`.
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
| `net` | formula, `created - done - dropped` | signed / gray / down | Daily flow balance; the headline is the window total, and the delta reads shrinking/growing/flat from that same total. |
| `review_closed` | checklist, tags gtd+post, match Morning review, bool | bool / violet / up | The `#gtd #post` Morning review chore completed on D. |
| `dropped` | tasks, dropped, recurring exclude | count / gray / neutral | Non-recurring CANCELLED tasks cancelled on D, same exclusions as Done. |
| `habits` | checklist, tags gtd+pre, ratio | ratio / green / up | `#gtd #pre` chores met on D over chores due on D. |
| `keep_rate` | journal, keep_rate, pending+next | ratio / violet / neutral | Keep share of PENDING/NEXT answers on D. |
| `review_time` | pomodoro, minutes, review only | minutes / orange / neutral | Minutes attributed to the GTD review row, including `^gtd` task shares inside work sessions. |
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
      in: [sase]
    sase_blog_time:   # measures blog time on its own
      label: SASE blog time
      kind: pomodoro
      measure: minutes
      in: [sase_blog]
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
(including plugin-only `bob_path` and `day_strip`) all fall back to
defaults: range `4w`, the six-metric overview, no hides, no global since.
A custom entry whose ID matches a built-in replaces the whole definition
and is marked `redefined`; an invalid replacement removes that metric with
a warning instead of silently falling back.

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
reason, and dimension parts (`area`, `root_parent`, `project`, `theme`,
`kind`, `note`, `tier`, `outcome`, `met`/`missed`). Pomodoro and task
metrics add `area`, `root_parent`, and `project` homes from the shared
resolver in [pomodoro-homes.md](pomodoro-homes.md). `root_parent` is the
additive working-time grouping by root parent; every metric that carries
`area` and `project` parts also carries it, in grouped and prior points too,
with no `other` collapse.
The `theme` dimension splits a named session on the same `+` components as
the plan budget; catalog identity and hues are in
[pomodoro-themes.md](pomodoro-themes.md). Minutes-mode days also carry
`apportioned`, the share of each part that came
from sessions with two or more keys in that dimension. Consumers never
recalculate a display string, summary, percentage, or delta — the report
carries them all.

The JSON report includes `theme_hues` beside `homes`, keyed by normalized
theme key, for the theme dimensions present in the report. Values use the
shared `ThemeRef` fields (`key`, `name`, `hue`, `hue_source`). Human output
adds a colored `■` before known theme parts and a muted `□` for exempt themes;
`NO_COLOR` keeps the existing text unchanged.

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
include it. Day grouping still shows it as the trailing point. An unmet
provisional boolean point (`value: 0`) displays `not yet`; met points still
display `met`, and missing values remain `–`.

**Journal gaps.** `review_closed` is evaluated as a dependency even when
hidden or unselected, so a known closed-review day with no journaled event
in any tier from any host reports `null`/`missing` plus a `journal_gap`
warning — never a silent zero. PRE/POST-only days carry real events, so
Reviewed reads a real zero there. Before the journal started, days stay
`not_tracked` rather than flagging plugin gaps. If `review_closed` itself is
invalid, journal metrics report a `journal_limitation` instead.

**Health.** Each report carries `health.tasks` (`invalid_dates`,
`closed_without_date`, and `missing_created` when nonzero — every affected
task counts once per limitation, never once per metric or day),
`health.journal` (tracking start, malformed-line count, and per-host event
counts with the latest timestamp and day), and `health.homes` (in-window
`unlinked_minutes` / `unlinked_sessions` / `unfiled_links` /
`ambiguous_links` / `unresolved_links`, plus global
`area_color_invalid` and hue-collision lists). Matching
`warning <message>  <code>` lines use `area_color_invalid`
and `area_hue_collision`. `invalid_dates` counts a task
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
the daily median (`/day` suffix), _except_ `net`, whose headline is the window
total (`stat: "total"`, suffix `total`), bool is met days over days with data
(`–` when that is 0 of 0), and ratios pool across days. Totals, means,
linearly interpolated `p25`/`p75` (index `(n-1)*p`, half-away rounding),
`days_with_data`, `days_met`, the prior headline, and a delta round it out.
`summary.p25`/`p75` stay daily even when the chart is grouped. Parts
summaries rank dimensions by total; only `note` and `theme` collapse to top
five plus `other`. `area`, `root_parent`, and `project` keep every key (the
note tree already bounds them). Rolling medians span 7 day points, 4 week
buckets, or 3
month buckets, with a 7-day / 3-week / 2-month lead-in so the first displayed
point can fill, need half the window in real observations, and stay `null`
for today.

**Chart band.** Each metric includes `chart: {group, p25, p75}`. Quartiles
reuse the summary helper but run over displayed, non-null, non-provisional
**bucket values** (the same units as the bars: daily values, weekly/monthly
sums, pooled ratio buckets, or bool met-day counts). Today and rolling
lead-in never enter the band. Day grouping therefore matches
`summary.p25`/`p75`; week and month bands differ and `chart.group` names
which. An all-null window serializes `p25`/`p75` as JSON `null`.

**Daily points.** Grouped reports add `daily_points` for the requested
display window only, in the same `SeriesPoint` shape with daily displays and
quality. They exclude rolling lead-in and prior days and include today once
when the window includes it. Day-grouped consumers use `points`;
`daily_points` is omitted so the payload is not duplicated.

**Checklist presentation.** Daily checklist points (never aggregated
buckets) carry `checklist_state`: `met` (every due chore met), `missed`
(any due chore unmet, including a partial ratio), `not_due`, `missing`, or
`not_tracked`. Quality, reason, and numeric value stay unchanged. Checklist
metrics also add `chores` over complete visible days: stable key, met-day
count, due-day count, and display `N of M days`, with the full roster (no
top-five truncation). Today is excluded. A chore absent on a known day is
not due; globally missing or untracked points stay out of those
denominators. The daily grid still uses each point's `met`/`missed` parts.

**Part display.** Points and `daily_points` add `parts_display` beside
numeric `parts` (same dimension/key shape). Each value uses the existing
parts formatter: minutes as `29m` / `4h 05m`, other units as the raw count
string. Empty maps are omitted.

**Display.** Minutes render `4h 05m` / `45m` / `0m`, counts plain, signed
values `−6` / `+9` / `0`, bools `met` / `missed`, ratios whole percentages,
null `–`. Deltas read `≈ same` within ±1% (and when a ratio difference
rounds to 0 pts), multipliers at ≥2× or ≤0.5×, `new` against a zero prior
(sign −1 when the current value is negative), `–` against a null prior,
`▼ 100%` for a drop to zero, percentage points for ratios, days for bools;
sign-crossing uses a raw amount. `net` instead reads shrinking/growing/flat
by total sign. The net label describes the window itself, not a comparison
against the prior period. Every other delta names the prior period in words. Parts summary
values use minutes formatting only when the metric unit is minutes.

**Typical day.** Every current-window point on a day-grouped `count` or
`minutes` metric carries an optional `typical` object: the median of the
seven days before D (D−7…D−1, selected by calendar date so a gap can never
shift it), needing at least 4 usable days. Real zeros count; `missing`,
`not_tracked`, and provisional days are skipped. The rule is the one shared
with rolling medians, so `typical(D)` always equals the prior day's
`rolling_median`. The object names its window (`start`, `end`), the
usable-day count (`days`), the median (`value`, `display`), and the words
the Day strip paints: a `relation` (`above` | `below` | `equal`) that is
`null` on today, on days with no value, and on short history, plus ready and
short sentence builders (`words`, `detail`). Bool, ratio, and signed
metrics, week/month points, `prior_points`, and `daily_points` never carry
it; human and CSV output are unchanged.

`all` with no history uses the same end convention as presets (end =
yesterday, zero days) and still includes today as provisional.

## Schema-1 contract

Reports serialize with `ok: true`, `schema_version: 1`, ISO dates, the range
(with prior metadata), per-metric definitions/summaries/chart/points/parts/
warnings, health, global warnings, a top-level `homes` catalog keyed by
every home used in the report (`label`, `kind` project|area|bucket, `area`,
`subarea`, `path`, `hue`, `root_parent`; `project_label` on area-kind
entries, `short_label` on review-bucket entries, and every referenced root
present even when only a leaf carries minutes),
and a top-level `sessions` array when the
report includes at least one pomodoro metric. The catalog and `sessions`
are omitted when empty. `sessions` lists every closed Pomodoro in the
display window (today included) in chronological order. Each element is
`{date, path, line, time_range, minutes, minutes_display, quality, name,
homes}`: facts, not metric values, so themes and filters do not change
them. `homes` is the shared R9 object; its keys are in the catalog. On
the 2026-10-18 live vault, `bob stats -f json -r all` is 1,945,299 bytes
(1.90 MiB) with 1,093 session facts. Ratio points
add `num`/`den`, grouped points add `partial`, and day-grouped
`count`/`minutes` points in the current window add an optional `typical`
object (`start`, `end`, `days`, `value`, `display`, `relation`, `words`,
`detail`); it is absent on bool/ratio/signed metrics, on week/month points,
on `prior_points`, and on `daily_points`. Empty maps (`parts`,
`parts_summary`, `parts_display`, `apportioned`) and empty lists
(`daily_points`, `chores`) are omitted. `PartSummary` rows for `area`,
`root_parent`, and `project` add an optional `label` plus minutes-mode
`apportioned` / `apportioned_display`, with `root_parent` labels using the
bare catalog label. Points add an optional `apportioned` map,
`{dimension: {key: minutes}}`, omitted when empty.
`definition`, `limits`, `parameters`, `chart`, and `total` are always present
(total and chart quartiles may be JSON `null`). Optional fields that appear
when present: `since`, `reason`, `prior`, `delta`, `suffix`, `stat`,
`journal.since`, `missing_created`, `checklist_state`, `daily_points`,
`chores`, `parts_display`, `apportioned`, `label`, `project_label`,
`short_label`, `health.homes` lint lists, `typical`, and `sessions`.

Older schema-1 reports remain renderable: use `summary` quartiles only for
day grouping; omit an unavailable grouped band or calendar with a short
explanatory label; use numeric part text when `parts_display` is absent;
keep table access. Never fake daily history from grouped totals.

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
      "chart": {"group": "day", "p25": 1250, "p75": 3750},
      "points": [
        {"date": "2026-10-09", "end": "2026-10-09", "value": 0,
         "display": "0%", "quality": "ok", "provisional": false,
         "rolling_median": null, "num": 0, "den": 2,
         "parts": {"missed": {"journal": 1, "meditate": 1}},
         "parts_display": {"missed": {"journal": "1", "meditate": "1"}},
         "checklist_state": "missed"}
      ],
      "prior_points": [],
      "chores": [
        {"key": "journal", "met": 0, "due": 2, "display": "0 of 2 days"},
        {"key": "meditate", "met": 1, "due": 2, "display": "1 of 2 days"}
      ],
      "warnings": []
    }
  ],
  "health": {"journal": {"since": "2026-10-08", "malformed_lines": 1, "hosts": []},
             "tasks": {"invalid_dates": 1, "closed_without_date": 0, "missing_created": 1}},
  "warnings": []
}
```

A day-grouped minutes point adds `typical` (ready history shown here;
short history leaves `value` and `relation` `null` with `display` `"–"`):

```json
{
  "date": "2026-10-25",
  "end": "2026-10-25",
  "value": 205,
  "display": "3h 25m",
  "quality": "ok",
  "provisional": false,
  "rolling_median": 240,
  "typical": {
    "start": "2026-10-18",
    "end": "2026-10-24",
    "days": 7,
    "value": 250,
    "display": "4h 10m",
    "relation": "below",
    "words": "45m below the typical day of 4h 10m",
    "detail": "Typical day 4h 10m: median of Oct 18 – Oct 24 (7 of 7 days with data). This day is 45m below."
  }
}
```

**Errors** use a matching envelope (`ok: false`, `schema_version: 1`,
`error: {code, message}`). Vault I/O failures (unreadable vault) map to
CLI exit 1; configuration, request/unknown-metric, and non-Dataview
format failures map to exit 2. JSON errors print that envelope on
stdout; human and CSV errors print the message on stderr.

## `bob stats`

Read-only Daily workflow command after `ready`. Bare `bob stats` runs
the flag-only `overview` glance.

```text
bob stats [overview] [-b DIR] [-c] [-f human|json|csv] [-g auto|day|week|month] [-o N] [-r RANGE]
bob stats show <METRIC>... | -a   [same options]
bob stats metrics [-b DIR] [-f human|json]
```

Options (alphabetical; every long option has a short alias):
`-a/--all` (show: every visible metric), `-b/--bob-dir`, `-c/--compare`
(include the prior period's points; human `show` draws its ticks),
`-f/--format`, `-g/--group`, `-h/--help`, `-o/--offset`, `-r/--range`.

Environment: `BOB_CONFIG_FILE`, `BOB_DIR`, `BOB_NOW`, `NO_COLOR`.

Exit codes: 0 report; 1 vault I/O failure; 2 usage, unknown metric,
config error, or non-Dataview task format.

### Human output

Overview is a header, a sparkline table in each metric's 256-color hue
(orange 208, violet 141, green 36, blue 33, gray dim), today's
in-progress values, journal hosts, then `warning <message>  <code>`
lines. `·` marks null days; booleans use `●` met / `○` missed. Detail
(`bob stats show`) draws eighth-block columns, a hollow `▯` today
column, and a weekday × week grid for booleans. The boolean overview
sparkline uses `◌` for an unmet provisional today point. It prints every parts
dimension in a stable order: pomodoro `area`, `root parent`, `project`,
`theme`; tasks `kind`, `area`, `root parent`, `project`, `note`. Area,
root-parent, and project cells use catalog labels; under color they prefix a
`●` in the area's xterm hue, and buckets render dim. The project line shows
the top five homes plus
`other`, with buckets always shown outside those five. When apportioned
minutes are non-zero, a dim `split` line reports the share of project
minutes that came from sessions with several homes. Review bucket dots use
their area's hue; `@unlinked` and `@unfiled` stay dim. Reports with area or
project parts add the dim line `grouped by today's note tree · from Task Links`.
Metrics is a catalog table with a dim definition line per id.

Plain text under `NO_COLOR` or a non-TTY (`Styler::detect()`) drops the
glyphs.

### CSV

Tidy rows `date,end,metric,value,display,quality,provisional,num,den`
(empty `num`/`den` when not a ratio).

### JSON

One line. Overview and show print the schema-1 report. `bob stats
metrics -f json` prints definitions and parameters without points.

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
| M6 | review_closed | U | met | met | not yet·prov | Today's POST TODO in `gtd_daily.md` is due and unmet |
| M7 | dropped | U | U | 2 | 0·prov | `[!]` and `[-]` CANCELLED rows with cancelled dates |
| M8 | habits | U | 0/2 missed | 1/2 met | 0/2·prov | Skipped Journal, late Meditate, deferred Stretch; ratios pool 25% |
| M9 | keep_rate | M, no denom | G | 1/2 → 50% | 1/1·prov | Keep share with pooled basis points |
| M10 | review_time | U | 25 | 0 | 0·prov | `review: only`; Oct 9 `GTD + SASE` no longer counts; SASE-only today is zero |
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
Worked and done also assert hand-derived `typical` objects on every
displayed day — all short in this window (Oct 8 and 9 have no usable
history, Oct 10 has one day, today has two) — and bool `review_closed`
plus ratio `habits` points assert the `typical` key is absent, at both the
struct and the JSON level.

Writer vectors in `tests/fixtures/stats/review_answers.json` (R1–R24 plus
the `hosts` sanitizer vectors) pin the plugin outcome mapping, Card
after-line outcomes, host naming, and which recorded events contribute to
`reviewed`; navigation tests mirror the same cases. Reader vectors in
`tests/fixtures/stats/journal_reader_vectors.json` pin last-answer order
(instant, then path, then line — including when string order and instant
order disagree), duplicate keys, two hosts, malformed and truncated lines,
unknown outcomes/tiers, pre/post-only days, keep-then-done, gap days, and
pre-tracking dates.

## Homes fixture and H-vector table

`tests/fixtures/stats/homes_vault/` is a separate hand-computed vault for
derived Pomodoro homes. It leaves M1–M17 and `tests/fixtures/stats/vault/`
untouched. The clock is Saturday 2026-10-18 noon and the window is
`2026-10-15..2026-10-18` (complete Oct 15–17, provisional today Oct 18).
Expected values are derived by hand from the file contents and asserted in
`src/native/stats/homes.rs`. Home labels follow
[pomodoro-homes.md](pomodoro-homes.md): counted minutes C are the
theme-selected shares, then R5 splits C so every dimension sums to the
metric total.

| # | Metric | What it proves |
| --- | --- | --- |
| H1 | worked total 167m | Complete-window minutes; today 15m is provisional |
| H2 | worked area | gtd 45, dev 90, job 7, @unlinked 10, @unfiled 15; sums to 167 |
| H3 | worked project | sase 30, sase_blog 50, @review:GTD 35, bob 10, dev 10, job 4, job_apply 3, @unlinked 10, @unfiled 15; sums to 167 |
| H4 | apportioned | sase 10 and job 4 from multi-home sessions; gtd area 10 from HACK |
| H5 | sessions | 8 closed rows; lead homes put 1 on sase, 1 on sase_blog, and 2 on @review:GTD |
| H6 | done | 7 completions; 2 on sase, 1 @unfiled; job area tasks use `job · area tasks` |
| H7 | sase_time | themes:[SASE] counted C is 10m, then split onto sase/dev |
| H8 | health | 1 unlinked session / 10m, 1 unfiled link, hue collision gtd+body orange |
| H9 | sase_in | `in: [sase]` is 80m (blog 50 + HACK sase share 10 + GTD+SASE 20); review/unlinked never match |
| H10 | sase_blog_in | `in: [sase_blog]` is 50m on sase_blog — blog time measures on its own |
| H11 | exclude_sase | `exclude_in: [sase]` is 87m (167 − 80); review 35m and unlinked 10m stay |
| H12 | review_only | `review: only` is 35m on `@review:GTD` (named 25 + unnamed bridge 10) |
| H13 | sessions | 9 closed rows in the window including today; facts ignore metric filters |
| H14 | sase_count_in | `measure: count`, `in: [sase]` keeps 3 complete-window sessions when any linked task matches |
| H15 | review_time / review_count | Oct 14 work session splits 50m into 25m `@review:GTD` and 25m `bob`; review-only minutes and count keep the review share/session |
| H16 | worked `root_parent` | sase 80 (blog 50 + sase 30), `@review:GTD` 35, bob 10, dev 10, job 4, job_apply 3, `@unlinked` 10, `@unfiled` 15; sums to 167 |
| H17 | worked `root_parent` with `root: true` on sase_blog | root_parent sase 30, sase_blog 50; area, project, and `in:` values unchanged |

Oct 14 adds the separate H15 check: an unnamed 50m work session links the
Oct 15 `^gtd` row and `bob`; it is not treated as the same-day review bridge.
Oct 15: named GTD review 25m plus unlinked HACK 10m. Oct 16: BLOG 50m on
sase_blog, HACK 30m split bob/sase/dev, JOB 7m split 4/3,
MISC 15m on an article (@unfiled), GTD+SASE 20m all on sase (theme remainder
gives SASE 10m), unnamed 10m review via `[[#^gtd]]`. Oct 17 is an empty
Pomodoros section (real zero). Oct 18 (today) BLOG 15m is in `sessions` and
provisional on metrics. Repeated `compute` calls return byte-identical
JSON. H17 copies that fixture with `root: true` added to `sase_blog.md`.

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
- Until the first counted `]s` answer, `.bob/stats/review/` does not
  exist and Reviewed / Keep rate stay `not_tracked` ("no review answers
  have been recorded") rather than a silent zero. On 2026-10-11 that was
  still true on apollo, athena, and the MacBook.
- A live-vault snapshot on 2026-10-11 reported
  `health.tasks.invalid_dates: 1`, `closed_without_date: 230`, and
  `missing_created: 647`. Those counts are vault data quality, not
  engine errors; they do not change the headline metrics.
- Release-mode `bob stats` on this vault is about 340–370 ms, above the
  original ≤300 ms target. See [Performance](#performance).

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
besides Markdown. An opt-in parity test
(`BOB_STATS_REAL_VAULT_PARITY=1` with `BOB_NOW`, vault from `BOB_DIR` or
`~/bob`) re-checks sessions, worked, done, created, and dropped against
independent line-regex oracles on every live run and also runs
`bob stats show worked -c -f json`. On 2026-10-11 it reconciled
`done` Oct 10 exactly (engine 37 = Dataview 45 − 7 recurring − 1 daily
`^gtd` row).

Rollout measurement (apollo, live `~/bob`, HEAD `efc5593`,
`BOB_NOW=2026-10-11 12:00:00`, `NO_COLOR=1`, release binary):

| Path | Time |
| --- | --- |
| Engine 7-day five-metric `compute` (parity test) | 371 ms |
| `bob stats` (human overview) | 340 ms |
| `bob stats -f json` (stdout `.ok` true) | 370 ms |
| `bob stats show worked -c` | 360 ms (CLI parity test 350 ms) |
| `bob stats metrics` | 340 ms |
| `bob stats -f csv` | 360 ms |

CLI wall time is within ~20 ms of the engine pass. Earlier debug-test
numbers (~2.4 s) and a landing-fixes 573 ms read were measurement
artifacts; the land-agent release figure at `f9324da` was 365–378 ms
and matches this rollout. The original ≤300 ms target is not met on
this vault (~15–25% over). The extra time is the Markdown pass, not
journal I/O (no journal files exist yet). Treat 370 ms as the recorded
budget unless a later pass shrinks the index.

Homes-parts measurement (athena, live `~/bob`, this checkout,
`BOB_NOW=2026-10-18T12:00:00`, `NO_COLOR=1`, release
`bob stats -f json -r 4w`, nine runs after one warmup, Python
`perf_counter`):

| Path | Time |
| --- | --- |
| Before (4w JSON baseline) | 174 ms (plan); 168 ms median on this host |
| Homes-parts after (median / min / max) | 200 ms / 193 ms / 209 ms |
| Stats-drilldown final release check (median / min / max) | 182.2 ms / 178.5 ms / 188.1 ms |

The final stats-drilldown median is 4.7% above the 174 ms plan baseline,
within its 15% budget (200.1 ms). The measured 4-week JSON is 220,827 bytes.
The same clock's `-r all` JSON is 1,945,299 bytes (1.90 MiB; 1,093
closed sessions).

Root-parent measurement (athena, live `~/bob`, this checkout,
`BOB_NOW=2026-10-18T12:00:00`, `NO_COLOR=1`, release
`bob stats -f json -r 4w`, nine runs after one warmup, Python
`perf_counter`):

| Path | Time |
| --- | --- |
| Before (clean base, median / min / max) | 179.1 ms / 171.9 ms / 188.6 ms |
| Root-parent after (median / min / max) | 177.5 ms / 170.4 ms / 181.4 ms |

The after median is 0.9% below the before median, within the 10% budget.
The measured 4-week JSON is 82,632 bytes on this vault window.

Typical-day measurement (athena, live `~/bob`, this checkout,
real clock 2026-10-26, `NO_COLOR=1`, release
`bob stats -f json -r 4w`, nine runs after one warmup, Python
`perf_counter`):

| Path | Time |
| --- | --- |
| Before (clean base, median / min / max) | 233.7 ms / 219.8 ms / 277.1 ms |
| Typical-day after (median / min / max) | 217.3 ms / 212.0 ms / 218.7 ms |

The after median is 7% below the before median (run-to-run noise either
way), within the 10% budget. The measured 4-week JSON grows from 291,466
to 320,133 bytes (+9.8%) for the additive `typical` objects.

## Obsidian Stats block

The `bob-stats` code block on `dash_stats.md` reads the same schema-1
report as `bob stats`. Open it from the dashboard STATS chip or the
Open Stats command; the page contract is in
[dashboard.md](dashboard.md#child-pages). The block keeps metric values,
labels, quality, definitions, and formatted parts from Bob; the plugin
only lays them out and manages the current view.

On desktop the block finds `bob` by checking `stats.bob_path` in
`~/.config/bob/config.yml` first (`~/` is expanded), then
`~/.cargo/bin/bob`, `~/.local/bin/bob`, `/opt/homebrew/bin/bob`, and
`/usr/local/bin/bob`, then `bob` on PATH. GUI Obsidian on macOS does not
inherit the shell PATH, which is why the fixed locations are tried first.
The error card lists exactly the paths that were tried.

The focus chart groups its bars by the report's selected day, week, or month bucket.
Its legend names that grouping and, when supplied, the 25th–75th percentile range for
those displayed buckets. The rolling-median line uses Bob's supplied daily rolling
median. Optional prior-period ticks align by elapsed position across the two windows.
Today is outlined as provisional; partial buckets, missing observations, and the
tracking start are labeled separately. Metric colors identify the selected series while
labels and values use Obsidian text colors.

The context panel follows the focused metric family:

- Pomodoro metrics open on **Where the time went**, grouped by root parent by
  default, with **Themes** one click away. The root bar reuses the area-bar component
  and is fed root rows in the same order as the Day strip's root-parent bar (area hue-slot
  order, then label, with buckets last in gray), with those same distinct root
  colours; root rows and their sub-project leaves match them and area headers keep
  the area hue; its `aria-label` reads
  `Root parent split: …`. Groups rank by area total from `area` parts. Each group
  shows its header row (dot, area link, `value · share`, `sessions N`), then up to 3
  root rows plus `N more` / `Show fewer`. The area's own root reads
  `<area> · area tasks` from the catalog's `project_label`. Root rows carry a label
  link, `value · share`, the apportioned mini bar, `sessions N` (sessions with any
  part whose catalog `root_parent` is this root), and fix-it copy for buckets.
  Multi-home minutes use a lighter tint for the apportioned share. A project root
  with at least one other leaf in the span offers a quiet `▸ N sub-projects` toggle
  (N excludes the root itself); expanded leaf rows come from `project` parts whose
  catalog `root_parent` is the root, ranked by value, each with a label link, a
  value, and a mini bar scaled to the root's value, and the root's own leaf reads
  `<root> · own tasks`. The toggle state is `homeSubsExpanded[root]` with focus key
  `panel-home-subs-<key>`, collapsed by default. `@unlinked` and `@unfiled` render
  as their header row plus fix-it copy only, never repeating themselves as a child
  row. The footer reads
  `▒ split from sessions with several root parents · grouped by today's note tree · from Task Links`.
  Opening the dash with a day pinned (`openStatsForDay({ panel: "homes" })`) selects
  the Root parents view, even right after a Themes visit. `openStatsForDay({ panel:
  "themes" })` selects the Themes view. Reports without
  `parts_summary.root_parent` fall back to the old `Areas & projects` leaf grouping
  with an `Update bob to group by root parent.` hint; a stale
  `homesView === "areas"` maps to `roots`.
- **Themes** remains one click away and shows the supplied theme breakdown. Theme
  bars and names come from the report's `theme_hues`. Reports
  without `root_parent` parts default to Themes and show an update-bob hint. Reports
  without the additive `sessions` list omit session drill-down controls.
- Root rows open their home note; Ctrl/Cmd opens a new leaf. Pomodoro rows can
  expand contributing sessions (newest first, 20 initially) with date, time range,
  duration, authored name, and home shares; selecting a session opens its daily note at
  the ledger line. Pinning a day or bucket switches the table and title to that supplied
  span. Count metrics use the same groups without color tint.
- Task metrics show the Created, Done, Dropped, and Net flow series when those metrics
  are present, followed by the focused task metric's `<Label> by root parent` counts
  with the same grouping and no bar.
- Review metrics show supplied answers by tier and outcome; Keep rate also shows its
  90% reference line and the review-time context.
- Checklist metrics show a daily state calendar and, when supplied, the full chore
  summary/grid. Filled cells mean met, outlined cells mean missed, blank cells mean not
  due, and crossed cells mean missing or not tracked.

The **Focus metric** selector includes every metric returned in the report, even when a
metric is not in the overview tiles. Selecting a tile or metric keeps the chart and its
family panel together. Range chips select a window; the previous/next controls move
through windows, **vs prior** requests the comparison series, **Custom…** selects
inclusive dates, and **Table** toggles the point table. These view-only changes reuse the
report already loaded for that selection. A range, offset, or comparison change loads
the corresponding report.

Hovering or focusing a chart column displays its value, date or bucket span, quality,
and top parts. Click or Enter pins the day; the separate **Open daily note** control
opens the selected existing day, and Ctrl/Cmd activation opens it in a new leaf. For a
week or month bar, the tooltip continues to describe the whole bucket. The day selector
under Day detail chooses a supplied daily point inside that bucket; grouped totals are
never presented as daily values. Left/Right moves the daily cursor, j/k changes the
focused metric, h/l shifts the window, t toggles the table, and Esc clears the pin or
closes the definition panel. Inputs and native controls keep their normal key behavior.

**Definition** discloses the metric's report-provided definition, unit, source, tracking
start, and limits. No definition or metric value is reconstructed by the renderer.

### Day strip

In desktop Obsidian, a Day strip renders as the first child of the
rendered `bob-plan` block in canonical daily notes (`YYYY/YYYYMMDD.md`),
with the `.bob-plan` badges directly after it, in Live Preview and
Reading view. It summarizes that note's date, called D below. Daily
notes without a `bob-plan` block show no automatic strip. The last painted
bar is the strip's last line, so it reads as
the separator above the badges. `## Pomodoros` carries no total; the
strip repeats it, so the heading stays bare.

Line 1 holds five facts (`worked`, `reviewed`, `done`, `created`,
`review_closed`, in that order). Each fact stacks three tiers: the
report's metric label (small, uppercased, muted), the D point value
(large, tabular), and a quiet sentence-case typical-day line that keeps
its space on every fact, boolean and skeleton included. Facts share the
line's width in columns split by hairlines and fold into three columns
below about 34em. In numeric displays, the unit letters are set smaller.
Numeric and word values show bob's `display` exactly. The typical line
paints bob's `typical` object and calculates nothing: an SVG ▲, ▼, or
`=` mark (when `relation` is `above`, `below`, or `equal` and D is not
provisional), the fixed word `typical`, and bob's typical `display`.
Today has no mark, so the text starts flush with the value. Short
history paints `typical –`. Missing, not-tracked, boolean, skeleton, and
older-bob facts keep a blank reserved line. The line is `aria-hidden`;
bob's `words` go on the fact's `aria-label` (`<label> <display>[ so
far], <words>` for ready and short facts) and `detail` is inserted into
the tooltip right after the date line. Boolean facts paint a status mark
chosen from the point: a green check for met, a red ✕ for missed, and
the yellow In Progress half-ring while today's review is still open.
Bob's `display` stays the accessible text and tooltip when there is no
typical sentence. When today's review flips from not yet to met on a
live, non-stale repaint of the same day, the check draws itself once.

| Situation | Reference line | Fact aria-label |
| --- | --- | --- |
| Finished day, above | `▲ typical 4h 20m` | `Worked 5h 00m, 40m above the typical day of 4h 20m` |
| Finished day, below | `▼ typical 4h 10m` | `Worked 3h 25m, 45m below the typical day of 4h 10m` |
| Finished day, equal | `= typical 25` | `Done 25, the same as the typical day of 25` |
| Today (provisional) | `typical 4h 10m` | `Worked 2h 10m so far, typical full day 4h 10m` |
| Fewer than 4 usable days | `typical –` | `Reviewed 60, no typical day yet` |
| D has no value | blank | unchanged (`Reviewed –`) |
| Review closed (bool) | blank | unchanged |
| Skeleton | blank | unchanged (`Worked, loading`) |
| Older bob (no `typical`) | blank | unchanged |

The top lane row carries the meta and the switch: one
`span[role=link]` with as many roots as fit the line in bar order — each
a hue dot, the legend label, and the value — then one `+N` counter in
their place, with the timestamp (`so far · HH:MM`, the fetch time of the
shown report, not the wall clock) right-aligned on today's note only.
The roots bar is a 4 px fully rounded `div` (`aria-hidden`) with one
segment per root, `flex-grow` set to minutes and 2 px gaps of note
background between segments, never track colour. Each segment's `title`
reads `<label> <display>`; the track (`--background-modifier-border`)
shows only when the bar is empty or in the skeleton. Below it, the
themes legend sits over the themes bar. The themes legend has no meta, so it gets the full width
when both lanes show; in themes-only view the themes row takes the top-row
position with the meta and the switch. There are no history marks, and no separate Stats
link: every fact is itself the link to Stats, and each bar is a second
mouse-only target for its legend's action. A metric hidden by
`stats.hide` is omitted from the strip, not faked.

A quiet switch at the right of the top lane cycles both bars → root
parents only → themes only. It speaks the strip's own marks: a `●` dot for
root parents and a `■` rounded square for themes, filled when shown and
hollow when hidden. It never moves between presses, and its tooltip names
the current view and the next one. The view is read from memory, never from
disk, during a paint; every switch repaints from the cache without spawning
`bob`. The switch paints only when both lanes are available; on a
zero-Pomodoro day or a day with no theme parts it is absent, and the shown
lane always matches the choice whenever the switch is visible. The skeleton
paints the lanes of the current view plus an invisible inert placeholder;
stale matches ready and the switch works. The group `aria-label` names only
the shown lanes. When the view changes on the same day, the newly shown lane
fades in softly over 160 ms; data repaints, day changes, first paints, and
skeleton → ready never animate.

Segments group D's `worked` minutes by root parent (see
[pomodoro-homes.md](pomodoro-homes.md)), in the same order as
the Stats page's "Where the time went" bar: area hue-slot order, then
label, with buckets last in gray. Every root parent in the bar gets its own
colour: the biggest root of each area keeps the reported area hue, and the
other same-hue roots take the next free palette hue. `unlinked` and `unfiled`
stay gray. A report without `root_parent` parts, from an older
bob, falls back to `parts.area`, and the copy then honestly says
"Time by area". The legend names as many roots as fit the line. It drops
the lowest-minute roots first, keeps bar order, and puts one `+N` counter
(whose `title` lists the hidden roots as `<label> <display>` in bar order)
in their place. It switches to names-only when that names more roots.
Legend items never shrink, so labels never ellipsize. The legend link's
`aria-label` and `title` read `Time by root parent: <label> <display>, …`,
listing every segment, and the group `aria-label` sentence uses the same
phrase.

Each legend item paints a `legendLabel`: the catalog entry's `short_label`
when bob supplies a non-empty one (review buckets carry `GTD` for compact
views), else the bare catalog `label` for the `root_parent` dimension (never
`project_label`, so `gtd · area tasks` reads `gtd`), else the row label. When
two or more of the day's segments share a `legendLabel` ignoring case, each
of them paints its full `label` instead. Tooltips and accessible sentences
keep full labels: each legend item's `title` reads `<label> <display>`, and
bar segment titles, the `+N` title, and the legend and group `aria-label`
sentences do too.

The theme lane splits the same `worked` minutes by Pomodoro theme. One
segment per positive key in D's `parts.theme`, displayed with
`parts_display.theme[key]`. Labels come from `report.theme_hues[normalize(key)].name`
when present, else the part key; `(unnamed)` and `other` keep their keys.
Coloured themes sort by minutes descending then label; gray segments
follow, with `(unnamed)` last. Colour is identity from Bob's theme hues
(`theme_hues`, else the plugin's `pomodoroThemeHueFor` fallback): a palette
hue, or gray for exempt, `(unnamed)`, and `other`. Among coloured
segments that share a hue, the largest keeps the exact hue, the second
is `color-mix(in srgb, <hue> 65%, var(--background-primary))`, and every
later one is `42%`. Gray is never shaded. `bob themes color` pins remove
a recurring collision. Each theme legend item is a rounded-square swatch
(the legend-dot footprint), the ALL-CAPS name, and the value. The fitter
is the same as the roots legend, independently. Titles read
`<label> <display>`; the themes legend's `aria-label`/`title` start with
`Time by theme`. When `report.sessions` lists a session dated D whose
name splits into two or more components, the title appends `Pomodoros
with several themes are split evenly between them.` The group
`aria-label` appends ` Time by theme: <label> <display>, ….`.

Bob owns every value: labels, values, part displays, definitions, and
the typical-day `display`, `words`, and `detail` all come from the
report. The strip derives its colours from the report's area hues, theme
hues, plus status inks for boolean marks. The typical line has no hue.
The strip writes its own fixed copy only (`so far`, `typical`,
`refresh failed`, `no Pomodoros`, `Day stats unavailable — …`, `Retry`)
plus accessibility sentences and date words derived from D.

Display states: today in progress shows provisional values with
`so far · HH:MM`, and an unmet closeout shows the yellow In Progress
half-ring. A past day shows values with no meta; a missed closeout shows
a red ✕ and a met closeout shows a green check. A stale cached frame
mutes the mark ink like the numerals. `not_tracked` shows `–`.
`missing`, or a report warning for D, shows `–` (or the display) plus a
small `⚠` beside the label; the mark still paints when the point has
one. The roots legend and bar are omitted if `worked` is absent, is not
measured in minutes, or has no nonempty `parts.root_parent` (or fallback
`parts.area`) anywhere in its daily series.
Otherwise a zero worked day with `quality: ok` shows an empty roots track and
`no Pomodoros`; the theme lane is absent. The theme lane is omitted whenever
`worked` is absent, is not in minutes, no daily point has a nonempty
`parts.theme`, or D has no positive theme part. With no bar or status text, the
roots row holds the meta alone.
A future daily note shows nothing: no element height, no spawn. The first
load paints a skeleton in the loaded geometry, including both empty bar
tracks and a round placeholder for the boolean fact. A failed refresh keeps the cached values, in muted text, with
`refresh failed` (today: `so far · HH:MM · refresh failed`). A failure with nothing cached shows
one muted line, `Day stats unavailable — <title>.`, plus a `Retry` link;
when bob is not found the line names `stats.bob_path`. Mobile and hosts
without process-launch support render no strip. The strip never shows
another date's result while loading: a report whose
`today` predates the local date is never shown as "so far".

Where it renders: a stable host as the first child of the rendered
`bob-plan` block — on first paint and every 5 s repaint, in both Live
Preview and Reading view, because both run the code-block processor. The
host node is never detached. With no host (strip off, future day,
non-daily note, dashboard), `.bob-plan` stays the first child as before.
Nowhere else automatically: not under the
dashboard's `bob-plan` block or any non-daily note, not in hover
popovers, embeds, or canvas nodes, not in source mode, and not on mobile.
A `bob-day-stats` code fence embeds a strip in another note, such as a
weekly note, with `day: YYYY-MM-DD` in its body. The date can be omitted
only in a canonical daily note, whose path supplies D. Otherwise the fence
asks for a daily note or an explicit date. An explicit fence and the
automatic strip can both appear in the same daily note.

Interaction: clicking a fact opens Stats with D pinned and that metric
focused; clicking the roots legend or the roots bar opens Stats with
`worked` focused, D pinned, and "Where the time went" on the Root
parents view revealed; clicking the themes legend or the themes bar
opens the same pin on the Themes view (`openStatsForDay({ panel:
"themes" })`). The switch is a `span[role=button]` and never opens Stats;
click, Enter, and Space advance the cycle once, and focus survives the
repaint. A plain click reuses an open Stats tab; Ctrl/Cmd-click and
middle-click always open a new tab. Enter and Space
activate facts, the legends, and the switch. `mousedown` never steals focus (each bar
and the switch call `preventDefault`), so vim keeps working.
Facts, the legends, and the switch are keyboard stops
(`tabindex="0"`); the bars have no tabindex: the strip contains no
`<button>` elements.

Layout: `.bob-day-strip` keeps `margin: .5rem 0 0` and
`container-type: inline-size`. `.bob-plan-has-day-strip > .bob-plan`
takes `margin-top: ~.7rem` and keeps `margin-bottom: 1rem`. The top lane
row sits `~.35em` below the facts, its bar `~.45em` below that row, and a
following lane row `~.6em` below the bar, with no card, border, background, shadow, or button.
The `.6em` gap follows a bar. Height budget at a 16 px base: at most 132 px when the facts fit on one
line (540 and 700 px), at most 198 px when they wrap once (410 px); single-lane
views hold at most 100 px one-line and 166 px wrapped.

Data: the strip reuses the cached default Stats report when that report
is day-grouped, its daily series covers every date D−7…D, and D is
before `report.today`, or D and `report.today` both equal today's date
on the device. Week- and month-grouped default reports (when
`stats.range` is 92 days or longer) never cover D, because their
`daily_points` do not carry `typical`; a cold-cache today fetch that
settles with such a report re-selects at once and fetches the explicit
`-g day` range.
With no cached report, today's strip requests the default report; past
days request `bob stats show -a -f json -g day -r <D-7>..<D>`.
That explicit range is also used when the cached default lacks coverage.
A report from before local midnight is not reused for its formerly
provisional day or a later date.

Refresh: cached values paint without running `bob`. Automatic strips wait
for a visibility notification before fetching; without an observer, they
check whether the block is connected and shown. Backgrounded or hidden
strips make no requests. A visible strip fetches missing data; cached
data refreshes after a vault change or ten minutes of age, with a minimum
age of 30 seconds for today and ten minutes for past days. Strips and the
Stats page sharing a cache key share one running request.

Controls: `stats.day_strip` in `~/.config/bob/config.yml` (`true` by
default) is the durable on/off for the automatic host — explicit fences
still render when it is `false`. The `Toggle Day stats` command toggles
automatic strips for the current plugin session. The switch and the `Cycle
Day stats bars` command set one view for every strip on the device; the view
is remembered on this device, and `stats.day_strip` and `Toggle Day stats`
still govern the whole strip. Opening or refreshing a
strip never changes note bytes and never appends journal events.

### Browser preview and fixtures

The fixture-driven real-browser preview exercises the production `bob-stats` block
renderer, SVG chart helpers, context panels, stylesheet, and event handlers. From the
bob-plugins checkout, run:

```sh
npm run build
node scripts/preview-stats-block.cjs --out /tmp/bob-stats-preview
```

The script discovers an installed headless Chromium, creates a standalone
`index.html`, captures the light/dark and narrow/wide matrix under the output directory,
and writes `matrix-report.md` with the browser and renderer versions plus interaction
checks. The HTML embeds the built renderer, stylesheet, and fixtures; it opens directly
from disk without a server. These preview outputs are not part of `npm test` or
`npm run validate`.

`scripts/fixtures/stats/README.md` records how to regenerate `canonical.json` from the
bob-cli golden vault with a pinned `BOB_NOW`. The `synthetic-*.json` files are clearly
labeled presentation-only cases for grouped buckets, signed values, medians, long labels,
and empty or all-null states. They do not describe vault data or validate metric
semantics.
