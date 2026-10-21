---
title: JSON outputs
description: Every bob machine-readable output, its flag, envelope shape, schema version, and contract link.
---

# JSON outputs

This page is the command-level lookup for machine-readable output. Each row
names the command, the flag that selects the shape, what the envelope
carries, the schema version the command actually prints (or explicitly
unversioned when the JSON carries no version), and the contract that owns the
semantics. Generated CLI pages own the exact flags; contracts own the
behavior.

Fixture prerequisites match the other reference pages: run the examples from
this checkout's `bob` in an isolated `HOME`/`XDG_*` tree, a synthetic vault,
`BOB_NOW=2026-10-20 12:00:00`, `TZ=UTC`, and `BOB_REF_JOBS_KICK=off`. Network,
clipboard, sync, and hook execution are stubbed or avoided. Stored file
versions are not command versions: freshness stored state and the review
journal carry their own numbers, and only the `schema_version` printed on
stdout counts below.

## Framing

JSON reports print to stdout. Warnings, progress, and hook chatter print to
stderr so a JSON pipe stays parseable. Usage errors exit 2.

Success and failure envelopes are per command family. Commands do not share
one `ok` / `error` / `schema_version` shape:

- Versioned helpers such as `capture-parse`, `plan`, `freshness`, and most
  `ref` commands print `ok: true` plus a payload on success. Many of those
  success envelopes carry `schema_version`.
- Capture-helper error envelopes are often `{"ok":false,"error":"…"}` and
  need not repeat the success `schema_version`.
- `bob capture -f json` is unversioned (`ok`, no `schema_version`).
- `bob vault-sync status -j` prints the status record directly, with no `ok`
  and no `schema_version`.
- A failing command may print JSON, a human message on stderr, or both; see
  that command's contract.

The `bob capture-parse` envelope below is a verbatim captured line from the
isolated fixture. The `bob plan` illustration after it is a shortened excerpt
of the schema-2 keys from a synthetic vault with no daily note yet; `…` marks
omitted nested numbers.

```bash
bob capture-parse -f json -- 'Buy milk @dev'
```

```text { .no-copy title="Output" }
{"ok":true,"schema_version":1,"input":"Buy milk @dev","body":"Buy milk","mode":"task","route":"dev","section":null,"block_id":null,"needs":[],"spans":[{"start":9,"end":13,"kind":"route"}],"diagnostics":[]}
```

```bash
bob plan -f json -b /path/to/synthetic-vault
```

```text { .no-copy title="Output (excerpt)" }
{"ok":true,"schema_version":2,"date":"2026-10-20","daily_file":"2026/20261020.md","status":"ok","caps":{"max_themes":…,"max_links":…,"max_next":…,"max_pending":…,"max_ready":…,"strict":false},"today":{"count":0},"today_tasks":[],"themes":{"count":0,"cap":…,"over":false},"links":{"count":0,"cap":…,"over":false},"next":{"count":0,"cap":…,"over":false},"pending":{"count":0,"cap":…,"over":false},"theme_names":[],"entries":[],"warnings":[]}
```

## Capture and protocol helpers

`bob capture` writes the vault. Most `capture-*` helpers are read-only
previews, pickers, and discovery calls for the Mac app and editors.
`capture-task-id` and `capture-pomodoro-name` write by default (`-d` /
`--dry-run` plans without writing). `capture-templates save`, `delete`, and
`clear` mutate the saved-template shelf; `list` and `show` are read-only.

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob capture` | `-f json` | One result per item: routing, task line, placement, plus batch `captures`, `warnings`, `plan_budget`, `pomodoro_blocks`, `task_blocks`, `open_notes`, and `pull_in` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-parse` | `-f json` | Per-item `mode`, spans, diagnostics, and `needs` for incomplete input | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-complete` | `-f json` | Completion context plus candidates and replacement ranges | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-pomodoro-name` | `-f json` | Name write (or `-d` plan) for one open Pomodoro | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-pomodoros` | `-f json` | Ledger entries for the day plus `warnings` | `schema_version: 1` | [Pomodoro homes](../pomodoro-homes.md) |
| `bob capture-related` | `-f json` | Candidate matches, UTF-8 draft anchors, verified actions, and already-linked tasks; `--query` adds scores | `schema_version: 1` | [Capture](../capture.md#bob-capture-related) |
| `bob capture-rewrite` | `-f json` | `input`, rewritten `text`, `changed`, `cursor`, `rule`, and `edits` | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-sections` | `-f json` | `route`, `count`, and non-`Tasks` `sections` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-targets` | `-f json` | `bob_dir`, `count`, and route `targets` | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-task-id` | `-f json` | ID write (or `-d` plan) plus `note_path` and picker `*_replacement` strings | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-task-sections` | `-f json` | ALL-CAPS child sections of one task | `schema_version: 1` | [Capture](../capture.md) |
| `bob capture-tasks` | `-f json` | Open tasks of one note with block IDs and status names | Unversioned (`ok`, no `schema_version`) | [Capture](../capture.md) |
| `bob capture-templates list` | `-f json` | Saved-template shelf entries | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates save` | `-f json` | Saved definition plus shelf position | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates show` | `-f json` | One saved definition | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates delete` | `-f json` | Removed entries plus remaining count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |
| `bob capture-templates clear` | `-f json` | Cleared count | `schema_version: 1` | [Capture](../capture.md#saved-templates) |

## Plan, readiness, freshness, and stats

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob plan` | `-f json` | Date, status, caps, Today entries, lane counts, and `warnings` | `schema_version: 2` | [Plan](../plan.md) |
| `bob ready` | `-f json` | Per-note Ready lane against `max_ready_per_note` | `schema_version: 1` | [Plan](../plan.md) |
| `bob freshness` and `bob freshness list` | `-f json` | Review queue report | `schema_version: 14` | [Freshness](../freshness.md) |
| `bob freshness seed` | `-f json` | Seed dry-run report: stamped counts, buckets, skipped | `schema_version: 14` | [Freshness](../freshness.md) |
| `bob stats overview` | `-f json` | Weekly glance report | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats show` | `-f json` | One or more metric details | `schema_version: 1` | [Stats](../stats.md) |
| `bob stats metrics` | `-f json` | Metric id, kind, and definition catalog | `schema_version: 1` | [Stats](../stats.md) |

Do not copy the freshness `14` onto stored files: it is the command
envelope version. The freshness contract owns the stored review state.

## Tasks, reconciliation, and queries

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob task move` | `-f json` | Move plan and repaired-link report | `schema_version: 1` | [Task move](../task-move.md) |
| `bob task reconcile` (aliases `task-status-hooks`, `mark-next-tasks`) | `-f json` | Sync result: marked, cleared, blocked, dependency, and reference changes | Unversioned (serialized sync result, no `schema_version`) | [Task Status Hooks](../task-status-hooks.md) |
| `bob task reroll` (alias `randomize`) | `-f json` | Re-roll plan and per-task outcomes | `schema_version: 1` | [Randomize](../randomize.md) |
| `bob query` | `-f json` (default is `paths`) | `engine`, `query_kind`, `format`, `paths`, `result`, and `warnings` | Unversioned (no `schema_version`) | [Bob query](../dataview.md) |
| `bob articles format` | `-f json` | Article plans | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles list` | `-f json` | Article notes needing work | `schema_version: 1` | [Articles](../articles.md) |
| `bob articles outline` | `-f json` | Section outline of one note | `schema_version: 1` | [Articles](../articles.md) |

`bob task archive`, `bob projects`, and `bob nightly` are human-only in this
checkout: they expose no `--format json` or `--json` flag.

## References, jobs, Keep, plugins, and status

| Command | Flag | Shape | Version | Contract |
| --- | --- | --- | --- | --- |
| `bob ref find` | `-f json` | Library verdicts and title candidates | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref list` | `-f json` | Notes by reading state with dates and parents | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref show` | `-f json` | One note: reading state, dates, and annotation pointers | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref scan` | `-f json` | Scan report: `mode`, `write_pdfs`, `hook`, `intake`, `summary`, `notes`, `failures`, `intake_only`, `queued`. `--intake-only` files queued `xlib/` PDFs and audio only (no annotation pass); `ok` is then also false when a writing drain leaves `queued` nonempty. Hard failures use the error envelope (`error.code` / `hint` / `paths`) instead of `summary`/`notes`/`failures`. | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [JSON output](../highlights-ref-sync.md#json-output) |
| `bob ref today` | `-f json` | Pulls one reference's reading task into a Pomodoro (`mode` `dry_run` or `write`; `-d` plans without writing). Success carries `changed`, `ref`, `task`, `pomodoros`, and `capture`. This is not a listing of today's references. | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [bob ref today](../ref.md#bob-ref-today) |
| `bob ref rename` | `-f json` | Rename plan and rewritten-link report | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref migrate-tasks` | `-f json` | Migration plan for open ref tasks | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref migrate-zorg` | `-f json` | Zorg-record migration plan | `schema_version: 1` (`REF_SCHEMA_VERSION`) | [Reference library](../ref.md) |
| `bob ref jobs list` | `-f json` | Queued background jobs plus a `summary` | `schema_version: 1` | [Ref jobs](../ref-jobs.md) |
| `bob ref jobs run` | Human only | Clips pending jobs through the shared ingest | No JSON flag | [Ref jobs](../ref-jobs.md) |
| `bob gkeep` and `bob gkeep list` | `-f json` (default is `table`) | Keep inbox notes with counts and error hints | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep doctor` | `-f json` | Adapter and credential diagnosis | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-markers` | `-f json` | Marker-removal plan | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep migrate-tasks` | `-f json` | Link-repair plan for open tasks | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob gkeep pull` | `-f json` | Inbox import report | `schema_version: 1` | [Bob Gkeep](../gkeep.md) |
| `bob plugins` and `bob plugins list` | `-f json` (default is `table`) | Repo plugins with version, sync, and vault state | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob plugins sync` | `-f json` | Copied, skipped, and unchanged files with backups | Unversioned (`ok`, no `schema_version`) | [Bob Plugins](../plugins.md) |
| `bob vault-sync status` | `--json` / `-j` | Direct `StatusRecord`: `last_attempt_at`, `last_success_at`, `local_sha`, `remote_sha`, `files_committed`, `push_retries`, `duration_ms`, `conflicts`, `interrupted_merge_recovered`, `aborted_merge_head`, `halted_merge_head`, `interrupted_fast_forward_recovered`, `last_error` | Unversioned (no `ok`, no `schema_version`) | [Vault Git sync](../vault-git-sync.md) |
| `bob completion status` | `--json` / `-j` | Protocol, executable, and per-shell install state | `schema_version: 1`, `protocol: 1` | [Completion](../completion.md) |

## Other structured formats

These are public but are not JSON. They use their own flags on the same
commands.

| Format | Command and flag | Shape |
| --- | --- | --- |
| CSV | `bob stats overview -f csv`, `bob stats show -f csv` | Same report rows as JSON, comma-separated |
| TSV | `bob ref migrate-tasks -f tsv` | Migration plan rows for spreadsheets |
| Table | `bob plugins -f table`, `bob gkeep -f table` | Human tables; the default for those two families |
| Markdown | `bob query -f markdown`, `bob ref find -f markdown`, `bob ref list -f markdown`, `bob ref show -f markdown` | Rendered notes output |
| Paths | `bob query` default (`-f paths`) | Newline-separated vault paths |

Hidden adapters (Keep, web-clip, Dataview internals) and on-disk state files
(`.bob/`, `templates.json`, the Stats review journal, ref job spool files)
are not public command outputs even when they contain JSON.

## Related

- [Your first Bob day](../start/first-bob-day.md)
- [Capture syntax](capture-syntax.md) for the tokens these envelopes describe.
- [CLI reference](cli/index.md) for the exact flags of every command.
- [Configuration and environment](configuration.md) for the `BOB_*` names that
  select vaults, adapters, and plugin roots used above.
