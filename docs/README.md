# Command guides

Start with [Getting started](getting-started.md) for vault settings, a first
capture and session, and command effects. The [root README](../README.md)
covers installation, the command index, dependencies, and environment variables.
The guides below explain current behavior and provide detailed contracts.

For daily work, read [Capture](capture.md), [Plan and Ready caps](plan.md), and
[Freshness review](freshness.md). For the Obsidian interface, read
[Dashboard navigation](dashboard.md), the [Task Card](projects.md#task-card),
and [task date marks](date-marks.md).

| Guide | What it covers |
| --- | --- |
| [capture.md](capture.md) | Capture grammar, JSON, and picker commands (`bob capture`, parse, complete, discovery) |
| [completion.md](completion.md) | Shell completion: the runtime model, protocol 1, and what completes |
| [dataview.md](dataview.md) | `bob query` Dataview and Tasks |
| [dashboard.md](dashboard.md) | Dashboard Work / Review / Browse navigation and collection pages |
| [date-marks.md](date-marks.md) | Task date marks: calendar-label display contract for canonical task dates |
| [freshness.md](freshness.md) | Task freshness: review lease, placement, evaluation, and conformance vectors |
| [getting-started.md](getting-started.md) | Vault requirements, first capture/session, command effects, and troubleshooting |
| [gkeep.md](gkeep.md) | `bob gkeep` Keep inbox drain into Obsidian tasks |
| [highlights-clip.md](highlights-clip.md) | `bob ref clip` web article capture into Highlights intake PDFs |
| [highlights-create.md](highlights-create.md) | `bob ref create` Markdown, PDF, and URL targets |
| [highlights-ref-sync.md](highlights-ref-sync.md) | `bob ref` PDF intake and reference notes |
| [obsidian-sync-exclusions.md](obsidian-sync-exclusions.md) | Historical: Obsidian Sync folder-exclusion semantics, kept for reference now that the vault syncs through git only |
| [plan.md](plan.md) | Plan budget, Today, and lanes definition, `bob plan` JSON, lints, and conformance examples |
| [plugins.md](plugins.md) | `bob plugins` list and vault deploy |
| [projects.md](projects.md) | `bob projects` `^prj` lifecycle and schedules |
| [randomize.md](randomize.md) | `bob task reroll` bulk re-roll of due prioritized tasks |
| [ref.md](ref.md) | `bob ref find`, `bob ref list`, and `bob ref show` reference-library lookup and views: verdicts, identity, filters, and JSON |
| [task-dependencies.md](task-dependencies.md) | Task dependency links: Depends-On line contract and conformance vectors |
| [task-status-hooks.md](task-status-hooks.md) | `bob task reconcile` Pomodoro-driven task status |
| [vault-git-sync.md](vault-git-sync.md) | Git-only Bob vault sync operations, triggers, conflict copies, and bridge policy |

`bob <command> --help` is the concise usage source for that command, and
these guides follow it. The one-line labels in `bob --help` are a short
index.
