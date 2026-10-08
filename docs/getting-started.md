# Getting started

Bob is a CLI for an Obsidian vault that uses `#task` checkboxes and a daily
Pomodoro ledger. Basic capture and native queries run without desktop Obsidian.
Dashboard badges, review keymaps, and the Task Card are provided by the custom
Bob plugins; installing the CLI alone does not install that interface.
Ready, Next, and Pending name task statuses ("lanes"). Pending is another
name for In Progress (`[/]`), not a waiting or Blocked state.

## Install and select a vault

From a checkout, run `just install`. Run `just install-all` to also deploy the
Bob plugins (and, on macOS, Bob Mac Capture) from sibling checkouts (offering
to clone any that are missing), restarting a running Obsidian when the plugins
change (on macOS that restart posts a notification first). With Cargo alone, run:

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
| `[/]` | In Progress, also called Pending or WIP | `IN_PROGRESS` |
| `[?]` | Blocked | `ON_HOLD` |
| `[x]` | Done | `DONE` |
| `[-]` | Canceled | `CANCELLED` |

The CLI reads settings from
`.obsidian/plugins/obsidian-tasks-plugin/data.json`. Without that file, native
Tasks queries use the default emoji format; `bob freshness` and `bob ready`
refuse a non-Dataview format. Blocked transitions also require the compatible
`?` definition described in [task status hooks](task-status-hooks.md#derived-blocked-status)
(`bob task reconcile`; formerly `bob task-status-hooks`, still accepted).

Capture routes select root-level notes: `@work` writes `work.md`. Route names
are lowercased and accept ASCII letters, digits, `_`, and `-`; picker-discovered
filenames must already have lowercase stems. A missing target note
is created when needed, and an existing `## Tasks` section is the preferred
insertion point. Ordinary capture does not create that heading or give the
note an area/project type. The `:` task-link and `+` parent-task pickers
discover the inbox, area notes, and non-terminal project notes.
Give area/project notes the appropriate frontmatter `type`
(`"[[area]]"` or `"[[project]]"`) to include them in those catalogs and in
per-note Ready reports. Nested prerequisites use a separate path-aware `&`
syntax; see [dependency capture](capture.md#adding-prerequisites-with-).

For the desktop interface, use the [plugin deployment guide](plugins.md):
select a local `bob-plugins` source checkout, deploy with `bob plugins sync`,
then enable the installed plugins in Obsidian's Community plugins settings.
Review requires Bob Ledger Tools and Bob Navigation Hotkeys. Sync copies
plugin assets; it does not enable plugins or create dashboard notes.

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
An eligible bare public URL queues a reference job instead of creating a task;
use `bob capture --no-ref 'https://example.com/article'` to keep it in the inbox.
For several tasks, separate items with blank lines; later bullet lines within
an item become children. The batch is planned before it writes, so a failed
item aborts the capture. See [Capture](capture.md) for routing, clipboard,
scheduling, prerequisites, and JSON interfaces.

## Plan and run a session

Use today's daily note at `YYYY/YYYYMMDD.md` inside the vault (for example,
`2026/20261004.md` on October 4), or set `BOB_DAY_FILE` to its full path.
Create the note if needed. Add a Pomodoros section with an unindented open
entry like the one below; linking needs an open entry, and the bare `=3`
start needs an untimed `()` placeholder with no other timed session open.

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
to Next. A Task Link is the dedicated `[[mac_inbox#^outline]]` child bullet
in the daily note; the task itself stays in `mac_inbox.md`. `=3` starts the
queued session's timer and leaves the task Next; the close records its work
outcome. Units are five minutes, and bare `=` means 25 minutes.
Quote session tokens in Zsh. Linking and starting can also
be one transaction: `bob capture '@mac_inbox:outline=3'`.

When finished, preview `bob capture --dry-run '=x'` to see the session's
numbered Task Links and proposed outcomes, then submit one close command:

| Command | Outcome |
| --- | --- |
| `bob capture '=x'` | Close; ordinary plain links count as worked, eligible tasks become Pending, and their links carry to the next session |
| `bob capture '=*'` | Close; park all numbered links, eligible tasks become Pending, and none of those links carry to the next session |
| `bob capture '=!'` | Close and complete every numbered Task Link |
| `bob capture '=x1!2'` | Keep task 1 in progress, complete task 2, and defer the remaining links |

For this one-task example, use `=!` if the outline is done, or `=*` if it
still needs work but should leave today's queue. The numbered `=x1!2`
example requires at least two links. "Deferred" means carry the link to the
next session without changing the task's lane or scheduled date. Plain `=x`
also respects existing link markers: a trailing `#` defers a link, while an
embed (`![[note#^id]]`) requests completion. Unresolved or ineligible targets
can be skipped with warnings; check the capture report. See
[close outcomes](capture.md#closing-the-running-pomodoro) for the full rules.

Inspect `bob task reconcile --dry-run`, then run `bob task reconcile` to
reconcile statuses and clean up links. Next and Pending are sticky: unlinking
a task keeps its lane. The explicit release gesture in Obsidian is Alt+N.
Blocked is derived from open prerequisites and future task schedules.
With the cursor on a Pomodoro Task Link, Alt+[ / Alt+] toggles the linked
task between Next and In Progress (asking once for an optional Work Log
summary when moving back to Next); In Progress targets show a rendered ◐
mark in today's daily note.

## Review the backlog

```bash
bob plan
bob freshness list --limit 10
bob ready
```

`bob plan` reports today's themes and Task Links plus Next/Pending pressure.
`bob freshness` shows the due review queue in PRE → NEW → PROJECTS → PENDING → NEXT
→ TICKLER → REFERENCES → ROTTEN → POST order. `bob ready` reports each area's or
project's whole Ready lane, including NEW and ROTTEN; the dashboard READY
backlog filters out those review buckets and Today-linked work. Today means
linked under an open Pomodoro in the selected daily note; it is not a task
status. Today-linked tasks are also excluded from the freshness review queue,
except PRE/POST `#gtd` checklist rows.
Use `bob ready NOTE` for one note's
worklist, or `bob ready --check` to exit 3 when notes are crowded.

With the review plugins enabled in Obsidian, `[S` or `]s` from outside the
queue starts at PRE. Every gesture that answers the landed row advances the
walk once to the next remaining item in the same keystroke: Ctrl+Enter
completes (never crossing the PRE/POST boundary — Ctrl+Alt+F does),
Ctrl+Alt+F keeps, Alt+N releases, Ctrl+Shift+Enter picks a Pomodoro (↵ takes the current/next one), a
resolving Task Card commit moves on. On an inbox task, every non-closing Task Card answer and
Ctrl+Shift+Enter asks where the task goes as the last step before writing, then answers and moves there.
Ctrl+Shift+M never advances: it follows
the moved task to its destination note, and the next `]s` resumes the walk.
Alt+F is the one answer that stays, `]s` skips,
and `<C-o>` returns to the row just answered. Walk commitments
with `]s` / Ctrl+Alt+F until **Commitments done**. `]S` jumps to POST; complete
Morning review last with Ctrl+Enter or Alt+F.
Ctrl+Alt+J/K also walks due tasks. Alt+F confirms a non-checklist task under
the cursor. `]s` / `[s` are the equivalent configured Vim bindings, not
shortcuts installed by the CLI. Ctrl+Alt+F confirms and advances
(Control+Option+F on a Mac). Hand edits alone do not stamp freshness.
`bob freshness list` only reports the queue; it does not confirm tasks.
`bob freshness seed` is a one-time migration for an
existing backlog; it is not the command to confirm today's new captures.
See [Freshness](freshness.md) for review gestures and tracker cadence.

Optional config lives at `~/.config/bob/config.yml`, selected by
`BOB_CONFIG_FILE` or `XDG_CONFIG_HOME`. [Plan](plan.md#config) documents caps;
[Freshness](freshness.md#2-fields-overrides-and-config) documents review
intervals. `p:1`–`p:4` capture and `bob task reroll` also require the
[priority configuration](projects.md#priority-property-and-scheduled-rolls).

## Save and read references

Bob's reference pipeline has three stages: capture a PDF into `xlib/`, scan it
into `lib/` and a note under `ref/`, then inspect or read that note. A bare
link capture handles the first stage in a background job:

```bash
bob capture --dry-run 'https://example.com/article'
bob capture 'https://example.com/article'
bob ref jobs
```

The preview reads local state and writes nothing. The submit queues the job;
the worker fetches the link independently of the capture process. Eligible
URLs contain no extra prose or capture markers. Short internal links, IP addresses, and the
default excluded hosts (`google.com`, `googleplex.com`, `youtube.com`,
`youtu.be`, `github.com`, `x.com`, `twitter.com`, and their subdomains) stay
tasks. Use `--no-ref` for a task, or configure the
[URL routing policy](ref.md#url-routing).

When the job reports `clipped`, preview and run the scan, then inspect the
reading queue:

```bash
bob ref scan --dry-run
bob ref scan
bob ref list
bob ref find 'https://example.com/article' --include-intake
```

`scan` processes the configured intake and library, so review its preview
before writing. Pending jobs and intake PDFs are absent from `ref list` until
they have a reference note. `find --include-intake` can locate a PDF awaiting
scan; `ref jobs` reports a link still being fetched. If background capture
fails, the worker writes the link as an inbox task with a warning and retry
command. See [ref jobs](ref-jobs.md) for recovery.

For a local PDF, Markdown report, or immediate URL import, use
`bob ref create <TARGET>`; it completes the import inline. Run `bob ref doctor`
to check dependencies. Local PDFs are stamped as-is; Markdown needs pandoc,
XeLaTeX, fonts, and LaTeX packages; web articles need `uv` and a browser.
See the [target guide](highlights-create.md). Lookup commands require the
configured `ref/` directory to exist.

To read a note's annotations and your own comments, use its path or an exact
identifier returned by `find` or `list`:

```bash
bob ref show ref/blogs/article.md
bob ref show ref/blogs/article.md --comments-only --format markdown
```

The path above is illustrative; use the actual note path Bob reports.

## What commands change

| Commands | Effects |
| --- | --- |
| `query`, `plan`, `ready`, `freshness list`, capture discovery/parse/complete, `projects list` | Read local vault state |
| `ref find`, `ref list`, `ref show`, `ref jobs [list]`, `ref migrate-zorg` without `--write` | Inspect local references, jobs, or a migration plan without vault writes |
| `capture`, `capture-task-id`, `capture-pomodoro-name`, `projects sync`, `task reconcile`, `freshness seed` | Write vault notes; bare reference links queue jobs that later write intake PDFs or fallback tasks; preview with `--dry-run` where offered |
| `vault-sync`, `nightly` | Reconcile the vault with Git, including commits, merges, and pushes |
| `task archive` | Archive task blocks and repair links; in a Git vault, commit touched files and push |
| `task reroll` | Re-roll schedules; in a Git worktree, sync before/after and commit rewritten notes; `--offline` skips sync but still commits locally |
| `plugins list`, `plugins sync` | Pull the plugin source repo by default, including for `sync --dry-run` (`--no-pull` skips); `sync` deploys plugin assets with backups |
| `gkeep list`, `gkeep pull` | Contact Keep; `pull` writes tasks or reference PDFs, commits task writes in a Git worktree unless `--no-commit`, then archives verified Keep notes unless `--no-archive`; `pull --dry-run` still contacts Keep |
| `ref create`, `ref scan`, `ref sync` | Write PDFs/reference notes; writing scans can run a configured pre-scan hook |
| `ref jobs run` | Fetch queued links for their stored destination vaults; write intake PDFs or fallback inbox tasks |
| `ref migrate-zorg --write` | Copy legacy reading records into `ref/zorg/` and commit them; sync before and after unless `--offline` |
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
| A native query using plugin-dependent dashboard filters omits tasks | Native queries do not load desktop Bob plugins; use `bob freshness` for the headless review queue |
| A captured link is absent from `ref list` | Inspect `bob ref jobs`; a clipped intake PDF needs `bob ref scan` before it has a note |
| A link stays a task | Extra prose, markers, children, a forced destination, `--no-ref`, an excluded host, or routing disabled in config |
| Markdown PDF creation fails | Check the `pandoc`, `xelatex`, and `latex_packages` rows in `bob ref doctor`, plus the required fonts |

Use `bob <command> --help` for accepted options and the [guide index](README.md)
for detailed behavior, output formats, and recovery steps.
