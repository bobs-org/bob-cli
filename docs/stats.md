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
