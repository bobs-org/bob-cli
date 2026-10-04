# Getting started

Bob is a CLI for an Obsidian vault that uses `#task` checkboxes and a daily
Pomodoro ledger. Basic capture and native queries run without desktop Obsidian.
Dashboard badges, review keymaps, and the Task Card are provided by the custom
Bob plugins; installing the CLI alone does not install that interface.

## Install and select a vault

From a checkout, run `just install`. With Cargo alone, run:

```bash
cargo install --path . --locked
bob completion install
```

See the [installation guide](../README.md#installation) for remote installation
and [completion](completion.md) for Bash/Zsh setup. Completion installation
does not edit shell rc files.

Select an existing vault directory and inspect it:

```bash
export BOB_DIR=/path/to/your/bob-vault
bob --help
bob capture-targets
bob projects list
```

The default vault is `~/bob`. Commands that accept `--bob-dir` let you override
it for one invocation. `bob` does not bootstrap a vault, its daily notes,
Obsidian settings, or Git remote.

## Vault requirements

Bob writes Dataview-style task metadata such as `[created::2026-10-04]` and
`[scheduled::2026-10-05]`. Configure Obsidian Tasks to use the **Dataview** task
format and the `#task` global filter. Its status registry should include:

| Checkbox | Bob meaning | Tasks type |
| --- | --- | --- |
| `[ ]` | Ready | `TODO` |
| `[*]` | Next | `ON_HOLD` |
| `[/]` | In Progress, also called Pending | `IN_PROGRESS` |
| `[?]` | Blocked | `ON_HOLD` |
| `[x]` | Done | `DONE` |
| `[-]` | Canceled | `CANCELLED` |

The CLI reads settings from
`.obsidian/plugins/obsidian-tasks-plugin/data.json`. Without that file, native
Tasks queries use the default emoji format; `bob freshness` and `bob ready`
refuse a non-Dataview format. Blocked transitions also require the compatible
`?` definition described in [task status hooks](task-status-hooks.md#derived-blocked-status).

Capture routes select root-level notes: `@work` writes `work.md`. Route names
are lowercased and accept letters, digits, `_`, and `-`. A missing target note
is created when needed, and an existing `## Tasks` section is the preferred
insertion point. Pickers discover the inbox, area notes, and non-terminal
project notes. Give area/project notes the appropriate frontmatter `type`
(`"[[area]]"` or `"[[project]]"`) to include them in those catalogs and in
per-note Ready reports. Nested prerequisites use a separate path-aware `&`
syntax; see [dependency capture](capture.md#adding-prerequisites-with-).

## Capture a first task

Preview the exact task, then write it:

```bash
bob capture --dry-run 'Write an outline @mac_inbox^outline'
bob capture 'Write an outline @mac_inbox^outline'
bob query --tasks '' --format markdown
```

The task lands in `mac_inbox.md` with a created date and `^outline` block ID.
New tasks have no freshness stamp: they await human confirmation. Use a unique
ID for each task in a note. Reusing this creation command does not edit the
existing task; a duplicate requested ID is an error.

For an ordinary task without an ID, `bob capture 'Buy milk'` uses the same inbox.
For several tasks, separate items with blank lines; later bullet lines within
an item become children. The batch is planned before it writes, so a failed
item aborts the capture. See [Capture](capture.md) for routing, clipboard,
scheduling, prerequisites, and JSON interfaces.

## Plan and run a session

Create today's daily note at `YYYY/YYYYMMDD.md` inside the vault, or point
`BOB_DAY_FILE` at the daily note you want to use. It needs a Pomodoros section
with a column-zero open entry, for example:

```markdown
---
parent: "[[day]]"
---
# Daily note

## Pomodoros

- [ ] () — WORK
```

Link the task created above to that session, then start it for 15 minutes:

```bash
bob capture '@mac_inbox:outline'
bob plan
bob capture '=3'
bob pomodoro
```

The marker-only `@mac_inbox:outline` links the existing task and raises Ready
to Next. `=3` starts the queued session; units are five minutes, and bare `=`
means 25 minutes. Quote session tokens in Zsh. Linking and starting can also
be one transaction: `bob capture '@mac_inbox:outline=3'`.

When finished, choose an outcome for the session's numbered Task Links:

| Command | Outcome |
| --- | --- |
| `bob capture '=x'` | Close using the ledger's existing worked/deferred classifications |
| `bob capture '=*'` | Close and park every numbered Task Link as In Progress without carrying it into the next session |
| `bob capture '=!'` | Close and complete every numbered Task Link |
| `bob capture '=x1!2'` | Keep task 1 in progress, complete task 2, and defer the remaining links |

Inspect `bob task-status-hooks --dry-run`, then run `bob task-status-hooks` to
reconcile statuses and clean up links. Next and Pending are sticky: unlinking
a task keeps its lane. The explicit release gesture in Obsidian is Alt+N.
Blocked is derived from open prerequisites and future task schedules.

## Review the backlog

```bash
bob plan
bob freshness list --limit 10
bob ready
```

`bob plan` reports today's themes and Task Links plus Next/Pending pressure.
`bob freshness` shows the due review queue in NEW → PROJECTS → PENDING → NEXT
→ RETURNED → REFERENCES → ROTTEN order. `bob ready` reports each area's or
project's whole Ready lane, including NEW and ROTTEN; the dashboard READY
backlog filters out those review buckets. Use `bob ready NOTE` for one note's
worklist, or `bob ready --check` to exit 3 when notes are crowded.

In Obsidian, `]s` walks review tasks and Alt+F confirms a task. Hand edits alone
do not stamp freshness. `bob freshness seed` is a one-time migration for an
existing backlog; it is not the command to confirm today's new captures.
See [Freshness](freshness.md) for review gestures and tracker cadence.

Optional config lives at `~/.config/bob/config.yml`, selected by
`BOB_CONFIG_FILE` or `XDG_CONFIG_HOME`. [Plan](plan.md#config) documents caps;
[Freshness](freshness.md#2-fields-overrides-and-config) documents review
intervals. `p:1`–`p:4` capture and `bob randomize` also require the
[priority configuration](projects.md#priority-property-and-scheduled-rolls).

## What commands change

| Commands | Effects |
| --- | --- |
| `query`, `plan`, `ready`, `freshness list`, capture discovery/parse/complete, `projects list` | Read local vault state |
| `capture`, `capture-task-id`, `capture-pomodoro-name`, `projects sync`, `task-status-hooks`, `freshness seed` | Write vault notes; preview with `--dry-run` where offered |
| `vault-sync`, `nightly` | Reconcile the vault with Git, including commits, merges, and pushes |
| `move-done-tasks` | Archive task blocks and repair links; in a Git vault, commit touched files and push |
| `randomize` | Re-roll schedules, commit rewritten notes, and sync; `--offline` still commits locally |
| `plugins list`, `plugins sync` | Pull the plugin source repo by default (`--no-pull` skips); `sync` deploys plugin assets with backups |
| `gkeep list`, `gkeep pull` | Contact Keep; `pull` writes and commits tasks, then archives verified Keep notes by default |
| `highlights create`, `highlights clip`, `highlights scan`, `highlights sync` | Write PDFs/reference notes; writing scans can run a configured pre-scan hook |
| `completion install`, `completion uninstall` | Change shell adapter files and the completion manifest |

Native vault reading and basic capture need no Git setup. Git workflows expect
the configured remote and credentials; `vault-sync` uses `origin/master`.
Follow the [Git sync runbook](vault-git-sync.md) before enabling maintenance.

## If a command refuses

| Symptom | What to check |
| --- | --- |
| Task absent from a picker | Root-level route, inbox/area/project type, project status, task status, and Tasks global filter; `&` has a broader catalog than `:`/`+` |
| Freshness or Ready reports reject the task format | Tasks must use `taskFormat: "dataview"`; missing settings fall back to emoji |
| Ready rejects invalid freshness config | `bob ready` shares the freshness scan; fix `freshness:` as well as `plan:` config |
| Linking fails | Daily note, Pomodoros heading, open entry, and a unique task block ID |
| Start says to finish the current Pomodoro | Close the running timed entry first; `bob capture '=x =3'` closes and starts atomically |
| Hooks refuse a Blocked transition | The Tasks registry's `?` status must match the required definition |
| Native dashboard queries omit NEW/ROTTEN | Plugin-dependent freshness gating needs Obsidian; use `bob freshness` for the headless review queue |

Use `bob <command> --help` for accepted options and the [guide index](README.md)
for detailed behavior, output formats, and recovery steps.
