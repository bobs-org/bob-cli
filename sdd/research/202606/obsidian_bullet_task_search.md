---
create_time: 2026-06-26
status: research
topic: Searching for particular bullets and tasks in Obsidian
---
# Research: Obsidian Bullet and Task Search

## Question

What is the best way to search for particular bullets or tasks in Bryan's
Obsidian vault, with enough precision to find the actual list item rather than
only the note that contains it?

## Executive Summary

Obsidian has several good partial answers, but no single native "search list
items as first-class rows" surface.

The best answer depends on the target:

- Use Obsidian's core Search for fast ad hoc text probes, especially when the
  exact phrase is known.
- Use the Tasks plugin for task-native searches that need status, dates,
  priority, tags, path filters, backlinks, and task editing/toggling in the UI.
- Use Dataview for structured task/list-item data, especially when searching
  task-line inline fields or ordinary bullets.
- Use `bob dataview` for headless task search and automation, but do not assume
  Bob's native Dataview subset can express every desktop Dataview `file.lists`
  query yet.

For Bryan's vault, the long-term "great" solution is a small Bob-specific
list-item search surface that treats bullets and tasks as indexed records. It
should use the same storage conventions already in place: `#task`, Tasks'
Dataview task format, Dataview inline fields, block IDs, path, heading/section,
line number, and task status.

## Local State Checked

Checked on 2026-06-26.

- Project memory says `~/bob` is Bryan's active Obsidian vault, and `ob`
  provides Obsidian Sync/headless support for local workflows.
- Enabled vault plugins include `dataview`, `obsidian-tasks-plugin`,
  `metadata-menu`, `quickadd`, `task-status-cycler`, `bob-project-tasks`, and
  other Bob plugins.
- Dataview is installed at `0.5.68`.
- Tasks is installed at `8.0.0`; the latest upstream release observed today is
  `8.2.2`, released 2026-06-22. The newer release does not change the high-level
  recommendation.
- Tasks settings use `globalFilter: "#task"` and `taskFormat: "dataview"`.
- Tasks auto-created, done, and cancelled dates are enabled.
- Custom task statuses are present for in-progress `/`, blocked `B`, and
  cancelled `-`.
- `bob dataview --format json --query 'TASK WHERE contains(tags, "#task") AND !completed LIMIT 3'`
  returns task rows with useful search fields such as `path`, `line`, `status`,
  `tags`, `text`, `section`, `blockId`, `outlinks`, and inherited page fields.
- The desktop Obsidian engine was not reachable from this shell:
  `obsidian` reported that it could not find a running Obsidian instance.

The local conclusion is important: Bryan already has the right task data model.
The gap is not storage. The gap is a polished search surface that works at the
list-item level.

## What "Good" Needs To Mean

A useful bullet/task search should support:

- text or regex over the item text;
- exact task rows, not just matching note files;
- open/done/in-progress/blocked/cancelled status filters;
- `#task` and other tag filters;
- path/folder filters;
- source heading or section filters;
- inline fields such as `[scheduled::]`, `[due::]`, `[p::]`, `[task_source::]`,
  `[source_page::]`, and `[id::]`;
- ordinary bullets as well as checkbox tasks;
- stable navigation back to the source line, block ID, or section;
- a good UI path for opening or editing a task;
- a headless path for scripting when GUI Obsidian is closed.

No off-the-shelf tool covers all of that cleanly.

## Option 1: Obsidian Core Search

Core Search is the fastest zero-install answer. It supports text, exact
phrases, boolean combinations, regex, and operators such as `path:`, `file:`,
`section:`, `block:`, `line:`, `tag:`, `task:`, `task-todo:`, and `task-done:`.
The official CLI also has `search` and `search:context` commands for vault text
search, plus task commands, when the Obsidian runtime is available.

Useful examples:

```text
task-todo:(#task "weekly review")
task:(scheduled:: 2026-06-26)
line:/^\s*[-*+]\s+.*weekly review/
block:(#task "source_page")
path:ref task-todo:("task_source:: highlights")
```

Strengths:

- already installed;
- interactive and fast;
- good when the search phrase is known;
- can distinguish todo and done tasks;
- good enough for occasional one-off lookup.

Weaknesses:

- result objects are still search hits, not normalized list-item records;
- filtering by Dataview/Tasks fields is textual rather than semantic;
- no first-class sort/group by due date, priority, heading, source PDF, etc.;
- ordinary bullet searches require regex or line/block tricks;
- not the best surface for recurring task dashboards.

Verdict: keep it as the quick lookup tool, but do not make it the main design.

## Option 2: Tasks Plugin Query Blocks

Tasks is the best interactive UI for checkbox tasks. It understands task status,
done state, dates, priority, recurrence, dependencies, path/folder filters, tags,
and description filters. It can render backlinks and provide edit/toggle
workflows.

Useful query shape:

```tasks
not done
description includes weekly review
tag includes #task
path includes prj
sort by due
sort by scheduled
```

For Bob, Tasks is especially relevant because the vault is already configured
with:

- `#task` as the global filter;
- Dataview task format;
- created/done/cancelled date tracking;
- custom statuses used by Bob workflows.

Strengths:

- best task-native UI;
- understands task fields instead of just text;
- can open/edit/toggle tasks from rendered results;
- saved query blocks make durable task dashboards.

Weaknesses:

- intentionally task-focused, not a general bullet search tool;
- query blocks are not a general command-palette fuzzy search;
- arbitrary Dataview inline fields are not always as natural as built-in Tasks
  fields;
- not the right headless automation API by itself.

Verdict: use Tasks for task dashboards and task editing. It should be one layer
of the solution, not the whole solution.

## Option 3: Dataview

Dataview is the best model for treating list items as data. Its docs describe
tasks and list items as having implicit fields, including status, checked state,
completion state, text, line number, section, tags, links, children, and block
IDs. It also supports task/list inline fields.

Task query example:

```dataview
TASK
WHERE contains(tags, "#task")
  AND !completed
  AND contains(lower(text), "weekly review")
```

Ordinary bullet query example for desktop Dataview:

```dataview
TABLE item.link AS Link, item.text AS Text, item.section AS Section
FLATTEN file.lists AS item
WHERE !item.task
  AND contains(lower(item.text), "weekly review")
```

Field-oriented task query:

```dataview
TASK
WHERE contains(tags, "#task")
  AND scheduled
  AND scheduled <= date(today) + dur(7 days)
  AND contains(lower(text), "review")
```

Strengths:

- can query tasks and ordinary bullets;
- exposes list item fields as data;
- fits Bob's existing Dataview inline-field conventions;
- can be used in notes as saved dashboards;
- `bob dataview` gives a headless path for many task searches.

Weaknesses:

- rendered Dataview views are weaker than Tasks for task editing;
- static query blocks are not as fluid as a real search UI;
- desktop Dataview and Bob's native Dataview engine are not identical;
- local testing showed Bob native `TASK` queries work well, but `file.lists`
  flattening for ordinary bullets is rough today.

Verdict: Dataview is the right semantic layer for list items. Use it directly
for dashboards and as the model for any custom search tool.

## Option 4: Omnisearch

Omnisearch is a strong full-text/fuzzy search plugin. Its README describes a
BM25-based search engine, typo tolerance, file-type filters, in-file search,
attachment/PDF/image indexing through Text Extractor, and a keyboard-first UI.
The latest release observed today is `1.29.3` from 2026-05-24.

Strengths:

- likely better than core Search for fuzzy full-text lookup;
- useful if the search problem includes PDFs, Office documents, OCR, or vague
  phrases;
- mature and popular.

Weaknesses:

- not installed in `~/bob` today;
- not task-native;
- does not make bullets/tasks into structured records;
- would add another search index and plugin dependency.

Verdict: install only if fuzzy full-text search becomes a separate requirement.
It is not the best primary answer for structured bullet/task search.

## Option 5: Obsidian Bases

Bases is increasingly important for note-level views, but it is not a good fit
for this problem. Bases is organized around notes, properties, formulas, and
views. The target here is a row per bullet or task line inside notes.

Verdict: useful adjacent tooling, but not the answer for bullet/task search.

## Option 6: A Bob-Specific List-Item Search Surface

A dedicated Bob search surface would close the gap between the existing tools:

- Core Search is fast but text-oriented.
- Tasks is task-native but does not cover ordinary bullets.
- Dataview has the right data model but not a polished interactive search UI.
- `bob dataview` gives headless task search but not full desktop Dataview parity
  for every list-item query.

The custom tool could be an Obsidian plugin command/pane, a `bob` CLI command,
or both. The Obsidian plugin is the better first UI because it can open the
source note at the selected item and delegate task editing to the Obsidian/Tasks
runtime. A CLI companion would be useful later for automation.

Suggested MVP:

- Command: `Bob: Search list items`.
- Search rows: every Markdown list item and task in the vault.
- Row fields: type (`bullet` or `task`), task status, checked/completed,
  `#task`, tags, path, heading/section, line, block ID, text, links, and
  Dataview inline fields.
- Query syntax: plain text by default, plus simple filters such as
  `type:task`, `type:bullet`, `done:false`, `status:B`, `tag:#task`,
  `path:ref`, `field:scheduled`, `scheduled<=today`, and quoted phrases.
- Result action: open source note at the line/block; for tasks, optionally open
  the Tasks edit modal or run the existing status-cycler command.
- Index source: start with Obsidian's cached Markdown metadata when running in
  Obsidian; use Dataview's API if it makes inline fields easier; fall back to a
  conservative Markdown parser only for a future headless CLI.
- Saved searches: allow named searches for common flows, such as open project
  tasks, highlight-derived tasks, unscheduled tasks, and bullets matching a
  topic under `ref/`.

This is not a large new data model. It is a better presentation/query layer over
the model Bryan already uses.

## Sources Checked

- Obsidian Search docs: https://obsidian.md/help/plugins/search
- Obsidian CLI docs: https://obsidian.md/help/cli
- Dataview task/list metadata: https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Dataview query types: https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/
- Tasks filters: https://publish.obsidian.md/tasks/Queries/Filters
- Tasks sorting: https://publish.obsidian.md/tasks/Queries/Sorting
- Tasks examples: https://publish.obsidian.md/tasks/Queries/Examples
- Tasks latest release: https://github.com/obsidian-tasks-group/obsidian-tasks/releases/latest
- Obsidian Bases docs: https://obsidian.md/help/plugins/bases
- Omnisearch README: https://github.com/scambier/obsidian-omnisearch
- Omnisearch community listing: https://community.obsidian.md/plugins/omnisearch
- Local Bob docs: `docs/dataview.md`, `docs/plugins.md`
- Prior Bob research:
  `sdd/research/202606/bulk_obsidian_task_properties.md`,
  `sdd/research/202606/hammerspoon_quickadd_task_capture_consolidated.md`,
  `sdd/research/202606/bob_obsidian_plugins_repo_consolidated.md`

## Recommended Solution

Use a layered solution now, and build a small Bob list-item search surface if
this becomes a frequent workflow.

Immediate workflow:

1. Use core Search for quick ad hoc lookup by exact phrase:
   `task-todo:(#task "term")` for tasks and `line:/^\s*[-*+]\s+.*term/` for
   bullets.
2. Create a saved Obsidian note of Tasks query blocks for task-native searches:
   open tasks, blocked tasks, scheduled tasks, project tasks, and tasks matching
   description/path/tag filters.
3. Create a companion Dataview note for structured list-item searches,
   especially ordinary bullets and task-line inline fields.
4. Use `bob dataview` for headless task searches and scripts where its native
   `TASK` support is sufficient.

Best durable solution:

Build a `Bob: Search list items` Obsidian plugin command that indexes bullets
and tasks as first-class rows, with filters for text, type, status, tags, path,
section, block ID, and inline fields. Keep Tasks as the task editing layer and
Dataview as the data/query model. Do not install Omnisearch for this specific
problem unless fuzzy full-text or attachment search becomes a separate goal.
