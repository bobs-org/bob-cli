---
create_time: 2026-06-26
status: research
topic: Consolidated research on searching particular bullets and tasks in Obsidian
---
# Research: Obsidian Bullet and Task Search

## Question

What is the best way to search for particular bullets or tasks in Bryan's
Obsidian vault, with enough precision to find the actual list item rather than
only the note that contains it?

## Consolidated Findings

There is no single stock Obsidian tool that is best for every bullet/task search.
The correct answer is a layered workflow:

- **Core Search** for quick interactive lookups when the phrase is known.
- **Tasks query blocks** for task-native filtering, sorting, grouping, and
  editing.
- **Dataview and `bob dataview`** for structured list-item/task data, ordinary
  bullets, inline fields, and headless automation.
- **Omnisearch** only if fuzzy full-text ranking or attachment search becomes a
  separate problem.

For the durable "great" solution, Bryan's vault needs a small Bob-specific
list-item search surface. It should treat bullets and tasks as indexed rows with
fields for text, type, status, path, section, line, block ID, tags, links, and
Dataview inline fields.

## Corrections To Prior Drafts

The two agent drafts mostly agreed, but the final consolidated position is:

- Native `bob dataview` can search ordinary bullets today via
  `FLATTEN file.lists`. Local verification returned bullet text, source path,
  zero-based line number, and section for matching bullets.
- Keep the caveat that Bob's native engine is a supported shell subset, not a
  promise of full desktop Dataview parity. For example, querying a flattened
  list item's `task` property as `L.task` currently collides with `TASK` parsing.
- Tasks custom statuses are under `statusSettings.customStatuses` in the local
  plugin settings: `/` is `IN_PROGRESS`, `B` is `ON_HOLD`, and `-` is
  `CANCELLED`.
- Do not center the recommendation on installing another search plugin. The
  local vault already has the core data model and query tools; the remaining
  gap is a better list-item-level search surface.

## Verified Local Context

Checked on 2026-06-26:

- Project memory says `~/bob` is Bryan's active Obsidian vault, with
  `ob`/obsidian-headless supporting local sync workflows.
- `~/bob` currently has 5,231 Markdown notes, excluding `.obsidian` and
  `.trash`.
- Core plugins relevant to this are enabled: `global-search`, `bookmarks`, and
  `bases`.
- Enabled community plugins include `dataview`, `obsidian-tasks-plugin`,
  `metadata-menu`, `quickadd`, `templater-obsidian`, `task-status-cycler`, and
  `bob-project-tasks`.
- Omnisearch is not installed.
- Dataview is installed at `0.5.68`; Tasks is installed at `8.0.0`.
- Tasks settings use `globalFilter: "#task"` and `taskFormat: "dataview"`.
  Automatic created, done, and cancelled dates are enabled.
- `bob` has no dedicated `search`, `find`, or `grep` subcommand today. The
  closest existing surface is `bob dataview`.

Local read-only checks:

```bash
bob dataview --format json --query '
TASK
WHERE contains(tags, "#task") AND !completed
LIMIT 2
'
```

This returned task rows with fields such as `path`, `line`, `status`, `tags`,
`text`, `section`, `blockId`, `outlinks`, and inherited page fields.

```bash
bob dataview --format json --query '
TABLE L.text, L.line, L.section
FROM "2026"
FLATTEN file.lists AS L
WHERE contains(lower(L.text), "obsidian")
LIMIT 2
'
```

This returned ordinary list-item matches from `2026/20260528.md`, including
text, source section, and zero-based line numbers.

## What Good Search Needs

A good solution should support:

- text and regex matching over item text;
- rows for exact bullets/tasks, not only matching note files;
- filters for task state, including todo, done, in-progress, blocked, and
  cancelled;
- tag, path/folder, and heading/section filters;
- task-line inline fields such as `[scheduled::]`, `[due::]`, `[p::]`,
  `[task_source::]`, `[source_page::]`, and `[id::]`;
- ordinary bullets as well as checkbox tasks;
- navigation back to the source line, block ID, or section;
- a UI path for opening/editing task results;
- a headless path for scripting when desktop Obsidian is closed.

No off-the-shelf tool covers all of this cleanly.

## Tool Survey

### Core Search

Core Search is the fastest zero-install answer for "where did I write that?"
lookups. It supports exact phrases, regex, boolean combinations, and operators
such as `path:`, `file:`, `tag:`, `line:`, `block:`, `section:`, `task:`,
`task-todo:`, and `task-done:`.

Useful examples:

```text
task-todo:(#task "weekly review")
task-done:(migrate)
line:(obsidian search)
block:(#task "source_page")
path:"2026/" task-todo:(invoice OR receipt)
```

Use bookmarks and embedded `query` code blocks for searches that need to be
rerun. This is still text search: results are hits, not normalized list-item
records, and Dataview/Tasks fields are matched textually rather than
semantically.

### Tasks Plugin

Tasks is the best interactive surface for checkbox tasks. It understands task
status, dates, recurrence, priority, dependencies, path filters, tags, and
description filters, and it can render editable/toggleable task lists.

Useful shape:

```tasks
not done
description regex matches /invoice|receipt/i
tag includes #task
path includes prj
sort by due
group by path
```

For Bob, this is especially strong because the vault already uses `#task` as
the global filter, the Dataview task format, date tracking, and custom statuses.
Its boundary is intentional: it is for tasks, not ordinary bullets.

### Dataview And `bob dataview`

Dataview is the best semantic model for list items. It exposes task/list fields
such as text, status, checked/completed state, tags, links, children, section,
block ID, and line number, and it reads task/list inline fields.

Task search:

```bash
bob dataview --format json --query '
TASK
WHERE contains(tags, "#task")
  AND !completed
  AND contains(lower(text), "invoice")
'
```

Plain-bullet search:

```bash
bob dataview --format json --query '
TABLE L.text, L.line, L.section
FROM "2026"
FLATTEN file.lists AS L
WHERE contains(lower(L.text), "obsidian")
'
```

This is the uniquely useful Bob layer: it runs headlessly, returns JSON, and can
feed scripts or future CLI commands. Its cost is DQL complexity, and native
mode is not exact full parity with every desktop Dataview edge case.

### Omnisearch

Omnisearch is a good fuzzy full-text search plugin. It is useful if the pain is
ranking, typo tolerance, in-file search, or attachment/PDF/OCR search. It is not
installed locally, is not task-native, and does not turn bullets/tasks into
structured rows. It should stay optional for this problem.

### Bases

Bases is useful for note-level views over properties, formulas, and saved
filters. It is adjacent, but not a solution for searching rows inside note
content. The target here is one result per bullet/task line.

## Decision Matrix

| Need | Best tool |
| --- | --- |
| Jump to a half-remembered bullet/task | Core Search |
| Save a recurring text search inside Obsidian | Bookmark or embedded `query` block |
| Build an editable task dashboard | Tasks query block |
| Filter tasks by status/date/priority/path/tag | Tasks query block |
| Search ordinary bullets as data | Dataview / `bob dataview` with `file.lists` |
| Search from terminal, cron, or scripts | `bob dataview` |
| Add fuzzy ranking or attachment search | Optional Omnisearch install |
| Get a polished row-level bullet/task search UI | New Bob list-item search command |

## Sources

- Obsidian Search docs: https://obsidian.md/help/plugins/search
- Obsidian CLI docs: https://obsidian.md/help/cli
- Dataview task/list metadata:
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Dataview query types:
  https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/
- Tasks filters: https://publish.obsidian.md/tasks/Queries/Filters
- Tasks sorting: https://publish.obsidian.md/tasks/Queries/Sorting
- Tasks examples: https://publish.obsidian.md/tasks/Queries/Examples
- Omnisearch README: https://github.com/scambier/obsidian-omnisearch
- Obsidian Bases docs: https://obsidian.md/help/plugins/bases
- Local docs: `docs/dataview.md`, `docs/plugins.md`
- Prior local research:
  `sdd/research/202606/bulk_obsidian_task_properties.md`,
  `sdd/research/202606/dataview_parity_consolidated.md`,
  `sdd/research/202606/bob_obsidian_plugins_repo_consolidated.md`

## Recommended Solution

Use the layered workflow immediately:

1. Use core Search for ad hoc lookups. Learn the small operator set that matters
   here: `task-todo:`, `task-done:`, `task:`, `line:`, `block:`, `section:`,
   `path:`, `tag:`, regex, `OR`, negation, and quoted phrases.
2. Persist recurring searches with bookmarks or embedded `query` code blocks.
3. Use Tasks query blocks for task-native dashboards, especially anything that
   needs status, due/scheduled dates, priority, grouping, sorting, or task
   editing.
4. Use Dataview and `bob dataview` for ordinary bullets, task-line inline
   fields, JSON output, and headless automation.
5. Do not install Omnisearch for this specific need unless fuzzy full-text
   ranking, attachment search, or typo tolerance becomes the actual bottleneck.

The best durable solution is to build `Bob: Search list items` as a small
Obsidian command/pane that indexes every bullet and task as a first-class row,
with filters for text, type, task status, tags, path, section, line, block ID,
links, and inline fields. It should open the source note at the selected item
and delegate task edits to the existing Tasks/status-cycler workflow. A later
`bob find` CLI wrapper can reuse the same query patterns for terminal
automation, but the Obsidian command is the right first UI because it can jump
to and edit the matched item directly.
