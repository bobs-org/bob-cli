---
create_time: 2026-06-04
status: research
topic: Consolidated research on improving Bryan's Obsidian usage
---
# Research: Improving Obsidian Usage

## Question

How can Bryan improve day-to-day use of the `~/bob` Obsidian vault without
disrupting the existing Markdown, Obsidian Sync, Dataview, Tasks, Templater,
and CLI-oriented workflow?

## Short Answer

The best improvements are incremental workflow tightening, not a new note
system:

1. Install `obsidian-vimrc-support` and move command-only Vim keymaps out of
   bespoke CM6 plugins.
2. Expand the already-working `refs.base` pattern into a few native Bases for
   review, triage, and note metadata hygiene.
3. Configure QuickAdd as the capture router, with Templater doing the note
   logic and Web Clipper handling browser capture.
4. Keep the current inline Tasks model, but use status types, grouping, group
   limits, postpone, and dependencies where they fit.
5. Keep `ob`/Obsidian Headless for server-side Sync, and use the official
   `obsidian` CLI only for desktop-app runtime workflows.

## Verified Local Context

Checked on 2026-06-04:

- Project memory says `~/bob/` is the Obsidian vault, `ob` is used for
  Obsidian Headless Sync, and new Markdown notes should include a `parent`
  frontmatter field.
- `~/bob` has 5,694 Markdown files; 5,669 have frontmatter delimiters; 313 have
  a top-level `parent:` line.
- Obsidian app config has Vim mode, line numbers, and automatic link updating
  enabled.
- Enabled community plugins include `dataview`, `obsidian-tasks-plugin`,
  `templater-obsidian`, `quickadd`, `task-status-cycler`,
  `obsidian-relative-line-numbers`, `note-refactor-obsidian`, and local custom
  plugins `bob-navigation-hotkeys`, `bob-ledger-tools`, and `block-id-prompt`.
- Core `bases` is enabled. The vault has `refs.base` and `Untitled.base`.
  `refs.base` is a real reference dashboard with formulas, multiple views,
  sorting, and grouping; `Untitled.base` is effectively a starter file.
- QuickAdd 2.12.3 is installed with zero configured choices.
- Templater uses `_templates`; the vault has useful `daily.md` and
  `schedule.md` templates.
- Tasks 8.0.0 uses `globalFilter: "#task"` and `taskFormat: "dataview"`;
  created, done, and cancelled date tracking are enabled. Custom statuses map
  `/` to `IN_PROGRESS`, `B` to `ON_HOLD`, and `-` to `CANCELLED`.
- `obsidian-vimrc-support` is not installed, and there is no
  `~/bob/.obsidian.vimrc`.
- Local `ob` is `0.0.8`; `npm view obsidian-headless version` reports
  `0.0.10`; local Node is `v22.14.0`, satisfying the current Node 22+
  requirement.
- `obsidian version` fails in this shell because the desktop app is not running,
  matching Obsidian CLI's documented app-runtime requirement.

## Finding 1: Consolidate Simple Vim Keymaps

The vault has several SDD prompts/tales for tiny Obsidian Vim ergonomics:
`obsidian_vim_o_list_continuation`, `obsidian_daily_vim_minus`,
`obsidian_backslash_daily_fallback`, `obsidian_file_link_caret_jump`,
`obsidian_alias_block_completion_cursor`, `obsidian_transclusion_toggle_keymap`,
and `child_note_keymap_dash`.

CM6 remains the right tool when a keybinding inspects editor state, computes
indentation, or places the cursor at a calculated offset. It is overkill when
the keybinding only runs an existing Obsidian command. The latter can move into
a single declarative `.obsidian.vimrc`.

| Existing feature | Keep CM6? | Reason |
| --- | --- | --- |
| `vim_o_list_continuation` | Yes | Inspects the line and computes inserted text/indentation. |
| `file_link_caret_jump` | Yes | Places the cursor at a computed offset. |
| `alias_block_completion_cursor` | Yes | Adjusts cursor placement after completion. |
| `daily_vim_minus` | Probably no | Command-style daily-note action. |
| `backslash_daily_fallback` | Probably no | Mostly command-style fallback behavior. |
| `transclusion_toggle_keymap` | No | Command toggle. |
| `child_note_keymap_dash` | No | Bob command binding. |

`obsidian-vimrc-support` supports `obcommand`, `exmap`, leader keys,
`surround`, `pasteinto`, and system clipboard yanking. Its `exmap` workaround
matters because CodeMirror's Vim mapping path passes only the first argument to
multi-argument commands.

Suggested first `.obsidian.vimrc` shape:

```vim
set clipboard=unnamed
let mapleader = " "

exmap save obcommand editor:save-file
nmap <leader>w :save<CR>

exmap surround_wiki surround [[ ]]
vmap <leader>l :surround_wiki<CR>

map <A-p> :pasteinto<CR>
```

Recommendation: install `obsidian-vimrc-support`, migrate the command-only
daily-note, transclusion, and child-note bindings first, and keep the
logic-heavy CM6 plugins until a specific Vimrc or `jsfile` replacement is
clearly simpler. Be cautious with Vimrc JavaScript support because it lets vault
files run code inside Obsidian and is disabled by default for good reason.

## Finding 2: Expand Bases From `refs.base`

One prior draft called Bases unused and another implied Bases lacked grouping.
Both need correction: `refs.base` is already useful, and current Obsidian Bases
docs support grouping by one property, formulas, summaries, multiple views, and
layouts including table, list, cards, and map.

The opportunity is not "replace Dataview." Use Bases where it is now strongest:
native, fast, visually editable dashboards over note files and frontmatter.
Keep Dataview and Tasks for richer dynamic logic and task-line metadata.

High-value Bases to add:

| Base | Purpose | Filter idea |
| --- | --- | --- |
| `inbox.base` | Triage metadata gaps | Missing `parent`, `type`, or `status`; recently modified notes. |
| `parent_review.base` | Navigate the note graph by topic | Group by `parent`; sort by `file.mtime`. |
| `active_work.base` | Review non-task active work notes | `status == "active"` or `status == "wip"`. |
| `reading_review.base` | Extend `refs.base` | `file.path.startsWith("ref/")` plus unread/review statuses. |
| `recent_notes.base` | Weekly review surface | `file.mtime > now() - "7 days"` and `file.ext == "md"`. |

The `parent` gap is especially actionable. Most notes already have
frontmatter, but only 313 have a top-level `parent:` line. A Base that exposes
missing `parent`, missing `status`, and recent edits would turn the memory rule
into a visible review habit without a bulk migration.

## Finding 3: Use QuickAdd, Templater, and Web Clipper as Capture Layers

QuickAdd is installed but unused, and Templater already has working daily and
schedule templates. That is a strong local signal: configure a small number of
choices instead of adding another large workflow plugin.

Good first QuickAdd choices:

| Choice | Type | Behavior |
| --- | --- | --- |
| `Capture inbox` | Capture | Append a timestamped item to today's daily note or an inbox note. |
| `New ref` | Template | Create a `ref/` note with `type`, `status`, `url`, `parent`, and title. |
| `New project note` | Template | Prompt for `parent`, `status`, and project link. |
| `New task` | Capture | Append `- [ ] #task ...` with optional `[scheduled::]`, `[due::]`, and `[p::]`. |
| `Schedule pomodoro` | Macro | Reuse `_templates/schedule.md` from a command or hotkey path. |

The recommended division of labor is: QuickAdd launches and prompts; Templater
computes note content; the Vimrc leader key runs the QuickAdd command inside
Obsidian.

QuickAdd also registers native Obsidian CLI handlers when Obsidian is at least
1.12.2 and QuickAdd is enabled:

```bash
obsidian vault=Bob quickadd choice="Capture inbox" value-text="..."
obsidian vault=Bob quickadd:list
```

That path is for a running desktop Obsidian session. For cron/server workflows,
prefer direct Markdown edits plus `ob sync --path ~/bob`.

Web Clipper is the browser-side complement. It can create new notes, append to
existing notes, or append to daily notes, and it exposes variables such as
`{{title}}`, `{{url}}`, `{{author}}`, `{{published}}`, `{{site}}`,
`{{description}}`, `{{selection}}`, `{{highlights}}`, and `{{content}}`.

Suggested Web Clipper templates:

```yaml
type: ref
status: unread
parent: "[[reading]]"
url: "{{url}}"
site: "{{site}}"
author: "{{author}}"
published: "{{published}}"
clipped: "{{date}}"
```

```markdown
- {{date}} {{time}} [{{title}}]({{url}})
  - {{selection}}
```

Use the first for reference notes under `ref/`; use the second for fast daily
append capture.

## Finding 4: Tune Tasks Instead of Replacing Tasks

The current Tasks setup is coherent: inline Markdown tasks, `#task` global
filter, Dataview task format, date tracking, daily-note review, and custom
status types. Do not migrate ordinary tasks to a note-per-task model unless
there is a bounded reason.

Useful Tasks features to apply now:

- Query by status type instead of checkbox symbol, because Bryan has custom
  statuses:

```tasks
(status.type is TODO) OR (status.type is IN_PROGRESS)
```

- Group and limit daily dashboards so each note/folder contributes only the
  next few visible items:

```tasks
not done
group by path
sort by function task.lineNumber
limit groups to 3 tasks
```

- Use file-property filters when a task dashboard should follow note-level
  metadata:

```tasks
filter by function task.file.property('project') === 'Project 1'
```

- Use the postpone button in Tasks query results for overdue scheduled/due/start
  dates, and hide it only where the query should stay read-only.
- Use task IDs and `depends on` only for small workflows where ordering really
  matters. The feature exists, but adding IDs to every task would be process
  overhead.
- Consider `delete` on completion for ephemeral daily checklist items, but not
  for durable project tasks where completion history matters.

TaskNotes is worth watching because it stores each task as a Markdown note with
YAML frontmatter and Bases-powered views. That is also a different model. If it
is ever tested, pilot it only for long-running project tasks that need calendar,
agenda, time-tracking, or API behavior, while keeping ordinary inline tasks in
Tasks.

## Operational Guardrail: Separate Headless Sync From App Runtime

There are now two command-line surfaces, and they solve different problems:

| Tool | Best use | Constraint |
| --- | --- | --- |
| `ob` / Obsidian Headless | Sync and Publish without desktop Obsidian | Does not run the desktop app plugin runtime. |
| `obsidian` CLI | App commands, daily notes, Bases, Tasks, QuickAdd, plugin commands | Requires Obsidian CLI support and a desktop app runtime. |

For Bob automation:

- Run `ob sync --path ~/bob` before filesystem scripts that inspect or mutate
  the vault.
- Upgrade `obsidian-headless` from local `0.0.8` to current `0.0.10` when ready,
  after checking `ob sync-list-local` and existing Sync config.
- Treat `obsidian ...` commands as app-session automation, not a headless server
  substitute.

## Prioritized Experiments

1. Install `obsidian-vimrc-support`; create `~/bob/.obsidian.vimrc`; migrate
   one command-style keymap.
2. Add `inbox.base` for missing `parent`, missing `status`, and recently
   modified Markdown notes.
3. Configure one QuickAdd `Capture inbox` choice and one `New ref` template.
4. Add a Web Clipper "Reading Queue" template for `ref/` notes.
5. Add one compact Tasks dashboard using `status.type`, `group by path`, and
   `limit groups to 3 tasks`.
6. Upgrade `obsidian-headless` after a quick Sync config check.

## Sources

- Obsidian Bases introduction: https://obsidian.md/help/bases
- Obsidian Bases views: https://obsidian.md/help/bases/views
- Obsidian Bases syntax: https://obsidian.md/help/bases/syntax
- Obsidian CLI: https://obsidian.md/help/cli
- Obsidian Web Clipper templates: https://obsidian.md/help/web-clipper/templates
- Obsidian Web Clipper variables: https://obsidian.md/help/web-clipper/variables
- Obsidian Headless: https://github.com/obsidianmd/obsidian-headless
- QuickAdd CLI: https://quickadd.obsidian.guide/docs/Advanced/CLI/
- `obsidian-vimrc-support`: https://github.com/esm7/obsidian-vimrc-support
- Tasks filters: https://publish.obsidian.md/tasks/Queries/Filters
- Tasks grouping: https://publish.obsidian.md/tasks/Queries/Grouping
- Tasks limiting: https://publish.obsidian.md/tasks/Queries/Limiting
- Tasks postponing: https://publish.obsidian.md/tasks/Editing/Postponing
- Tasks auto-suggest and dependencies: https://publish.obsidian.md/tasks/Editing/Auto-Suggest
- TaskNotes concepts: https://tasknotes.dev/core-concepts/
