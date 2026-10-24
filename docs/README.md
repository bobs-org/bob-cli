# Command guides

The public site at <https://bobs-org.github.io/> is the reader front door:

- [Get started](https://bobs-org.github.io/start/)
- [Guides](https://bobs-org.github.io/guides/)
- [Understand](https://bobs-org.github.io/concepts/)
- [Reference](https://bobs-org.github.io/reference/)
- [Develop](https://bobs-org.github.io/develop/)

Start with [Getting started](getting-started.md) for vault settings, a first
capture and session, and command effects. The [root README](../README.md) is
a short GitHub front door. The table below lists the in-repo behavior
contracts.

For daily work, read [Capture](capture.md), [Plan and Ready caps](plan.md),
[Freshness review](freshness.md), and [Daily Stats](stats.md). For the
Obsidian interface, read [Dashboard navigation](dashboard.md) (including
the STATS page), the [Task Card](projects.md#task-card),
[task date marks](date-marks.md), [task tag marks](task-tag-marks.md), and
[Today marks](today-marks.md).

For reading, start with the [reference-library guide](ref.md). Import material
with [`bob ref create`](highlights-create.md), or save bare links with
[`bob capture`](capture.md#saving-links-to-your-reading-queue). Background jobs
create PDFs, then attempt intake: filing PDFs and creating reference notes.
[`bob ref scan --intake-only`](highlights-ref-sync.md#intake-only-scan) retries
queued intake; full scans also synchronize annotations. The
[job guide](ref-jobs.md) explains progress, failures, and recovery.

<!-- BEGIN GENERATED: contract table -->
| Guide | What it covers |
| --- | --- |
| [articles.md](articles.md) | `bob articles` formatting, list, outline, adopt, and ArticlePlan JSON |
| [capture.md](capture.md) | Capture grammar, JSON, and picker commands (`bob capture`, parse, complete, discovery) |
| [completion.md](completion.md) | Shell completion: the runtime model, protocol 1, and what completes |
| [dataview.md](dataview.md) | `bob query` Dataview and Tasks |
| [dashboard.md](dashboard.md) | Dashboard Review / Work / Browse navigation and collection pages |
| [date-marks.md](date-marks.md) | Task date marks: calendar-label display contract for canonical task dates and recurrence rules |
| [freshness.md](freshness.md) | Task review queue, confirmation dates, recurring occurrences, returning tasks (ticklers), and JSON schema 14 |
| [getting-started.md](getting-started.md) | Vault requirements, first capture/session, command effects, and troubleshooting |
| [gkeep.md](gkeep.md) | `bob gkeep` Keep inbox drain into tasks or inline reference clips |
| [highlights-clip.md](highlights-clip.md) | Web article and collection capture into Highlights intake PDFs |
| [highlights-create.md](highlights-create.md) | `bob ref create` vault-note snapshots, Markdown, PDFs, and URL collections |
| [highlights-ref-sync.md](highlights-ref-sync.md) | `bob ref` PDF intake and reference notes |
| [obsidian-sync-exclusions.md](obsidian-sync-exclusions.md) | Historical: Obsidian Sync folder-exclusion semantics, kept for reference now that the vault syncs through git only |
| [plan.md](plan.md) | Plan budget, Today, and lanes definition, `bob plan` JSON, lints, and conformance examples |
| [plugins.md](plugins.md) | `bob plugins` list and vault deploy |
| [projects.md](projects.md) | `bob projects` `^prj` lifecycle, schedules, and reporting-only rollup |
| [pomodoro-homes.md](pomodoro-homes.md) | Derived Pomodoro homes, rollup, labels, chips, hues, and JSON contract |
| [pomodoro-themes.md](pomodoro-themes.md) | Pomodoro theme catalog, hues, hide journal, `bob themes`, and JSON |
| [randomize.md](randomize.md) | `bob task reroll` bulk re-roll of due prioritized tasks |
| [ref.md](ref.md) | Reference lookup, reading state, the Obsidian Reading Board, Today linking, renaming, JSON, and migration/rollback |
| [ref-jobs.md](ref-jobs.md) | `bob ref jobs` background capture progress, worker, fallback, and recovery |
| [stats.md](stats.md) | Daily Stats metrics, `bob stats`, JSON, the v1 review journal, and the Obsidian Stats page |
| [task-dependencies.md](task-dependencies.md) | Task dependency links: Depends-On line, BLOCKS mirror, successor offers, project-dependency contract, plus conformance vectors |
| [task-relations.md](task-relations.md) | Task relations: directed LINKS edges, the generated LINKED FROM mirror, and `~` / `~~` capture |
| [task-move.md](task-move.md) | `bob task move` selection, preservation, link repair, guarded writes, and sync behavior |
| [task-status-hooks.md](task-status-hooks.md) | `bob task reconcile` Pomodoro-driven task status |
| [task-tag-marks.md](task-tag-marks.md) | Task tag marks: the teal hash glyph that replaces the `#task` pill |
| [today-marks.md](today-marks.md) | Today marks: a display-only tomato on task lines linked under today's open Pomodoros |
| [vault-git-sync.md](vault-git-sync.md) | Git-only Bob vault sync operations, triggers, conflict copies, Keep receipts, and the Stats review journal |
<!-- END GENERATED: contract table -->

`bob <command> --help` is the concise usage source for that command, and
these guides follow it. The one-line labels in `bob --help` are a short
index.
