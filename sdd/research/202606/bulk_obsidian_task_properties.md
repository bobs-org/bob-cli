---
create_time: 2026-06-04
status: research
topic: Bulk setting the same property on Obsidian tasks across files
---
# Research: Bulk Obsidian Task Properties

## Question

Given a list of Obsidian tasks that live in different files, what is the best
way to set the same property on all of them in bulk?

## Short Answer

For Bryan's vault, the best answer is a small dry-run Markdown rewrite tool,
using `bob dataview` for task discovery:

1. Select the exact task rows with a Dataview `TASK` query, preferably through
   `bob dataview --format json`.
2. Normalize the selection to vault-relative path, Dataview line number, current
   task text/status, and optional block id.
3. Group candidates by file, re-read each source Markdown file, and verify that
   every selected line still matches the expected task.
4. Replace or append the target task-line inline field idempotently.
5. Show a dry-run diff by default, then write only on explicit `--apply`.

This is a task-line metadata problem, not a note-frontmatter problem. The right
format is a Dataview bracket inline field on the task line:

```markdown
- [ ] #task Follow up with Pat  [scheduled:: 2026-06-08]
```

GUI bulk-property plugins mostly operate at the note/frontmatter layer, and the
Tasks plugin UI is good for single-task edits but does not provide a documented
general "set this arbitrary property on these N tasks across files" command.

## Verified Local Context

Checked on 2026-06-04:

- Project memory says `~/bob` is Bryan's Obsidian vault, `ob` is available for
  headless sync, and Bryan has switched to Obsidian rather than active zorg use.
- `~/bob/.obsidian/plugins/obsidian-tasks-plugin/manifest.json` reports Tasks
  `8.0.0`.
- Tasks settings use `globalFilter: "#task"` and `taskFormat: "dataview"`.
- Tasks automatic created, done, and cancelled date tracking are enabled.
- Dataview is installed at `0.5.68`.
- `bob dataview --format json --query 'TASK WHERE contains(tags, "#task")'`
  returns task objects with `path`, `line`, `status`, `text`, `tags`, `link`,
  `section`, `blockId`, inline-field values, and inherited page fields.
- Dataview task `line` values from `bob dataview` are zero-based. For example,
  `2026/20260528_day.md` reported a task at `line: 28`; `nl -ba` showed that
  task physically on line 29.
- Bob task lines already use Dataview inline fields such as `[p::1]`,
  `[scheduled:: 2026-06-01]`, and `[completion:: 2026-05-28]`.

## Correction to Prior Drafts

One prior draft correctly noticed `generated_from_zorg` metadata in Dataview
task rows, but it over-applied that fact.

Dataview task rows inherit page frontmatter fields. Many older Bob pages still
have frontmatter such as:

```yaml
generated_from_zorg: true
zorg_source_abs: "/home/bryan/org/..."
```

Because of that inheritance, a task row can report `generated_from_zorg: true`
even if the specific task line was later edited or added directly in Markdown.
Local checks showed:

- `TASK` returned 10,606 task rows; 9,934 carried inherited
  `generated_from_zorg`.
- Restricting to explicit `#task` rows returned 372 rows; only 14 carried
  inherited `generated_from_zorg`.
- `~/bob/job.md` has `generated_from_zorg` frontmatter, but the checked Markdown
  task lines in `job.md` are newer than `/home/bryan/org/job.zo`, and the shown
  "Active Work" task lines were not present in that `.zo` source file.

So the final recommendation should not blindly route all such task edits to
`.zo` files. Treat `generated_from_zorg` as page-level migration context and a
warning signal, not proof that the selected task line's durable source is still
zorg.

Practical policy:

- Default to editing the current `~/bob` Markdown file, because project memory
  says the active workflow is Obsidian-native.
- If a selected file has `generated_from_zorg` frontmatter, add a guard in the
  tool: either warn, require an explicit override, or verify that the exact
  current task line still exists in `zorg_source_abs` before considering a
  source-file edit.
- Only build a `.zo` mutation path if there is a separate requirement to
  preserve or rerun the old zorg converter. If so, first verify converter
  round-trip behavior for `[key:: value]`.

## Property Format

For task-specific metadata, use Dataview bracket inline fields. Dataview's docs
state that task/list item metadata must use bracket syntax because the field is
not the only content on the line. Parentheses can hide the key in Reading view,
but Tasks writes Dataview fields with square brackets, so square brackets are
the safest default.

Tasks' Dataview task format uses these built-in field names:

| Intent | Field |
| --- | --- |
| Created date | `[created:: YYYY-MM-DD]` |
| Scheduled date | `[scheduled:: YYYY-MM-DD]` |
| Start date | `[start:: YYYY-MM-DD]` |
| Due date | `[due:: YYYY-MM-DD]` |
| Done date | `[completion:: YYYY-MM-DD]` |
| Cancelled date | `[cancelled:: YYYY-MM-DD]` |
| Priority | `[priority:: lowest|low|medium|high|highest]` |
| Recurrence | `[repeat:: every day]` |
| On completion | `[onCompletion:: keep|delete]` |
| Task id | `[id:: abc123]` |
| Dependencies | `[dependsOn:: abc123,def456]` |

For Bob-specific or custom metadata, use the same shape:

```markdown
- [ ] #task Example  [snooze:: 2026-06-10]  [p:: 1]
```

Spacing matters for Live Preview. Tasks' docs recommend separating adjacent
Dataview fields with either two spaces or comma-space; the Tasks modal writes
two spaces automatically.

Insert a new field before a trailing Obsidian block id, so the block id remains
the final marker:

```markdown
before: - [ ] #task Call Pat ^pat-call
after:  - [ ] #task Call Pat  [scheduled:: 2026-06-08] ^pat-call
```

Also note the `#task` global filter. Dataview can index inline fields on tasks
generally, but Tasks' Dataview-format parser only reads fields from task lines
that match the configured global filter. In Bryan's settings, that means
built-in Tasks fields should be on lines containing `#task` when they need to
participate in Tasks queries and UI behavior.

## Tool Survey

### Dataview and `bob dataview`

Dataview is the right selector. Its `TASK` query exposes task-level metadata,
including path, line, status, text, tags, section, links, children, and block id.
`bob dataview` makes this usable from the shell and returns structured JSON for
scripts.

Dataview is not the mutator. It can render tasks and support checkbox updates
in rendered views, but it is not a general bulk editor for arbitrary inline
fields.

### Tasks Plugin

The Tasks `Create or edit task` modal is the right UI for one task. Tasks docs
also document one multi-file side effect for dependency editing: saving
dependencies may add `id` fields to depended-on tasks and update `dependsOn`
fields. That is specific to dependencies, not a general arbitrary-property bulk
operation.

Tasks query blocks are useful to preview the selection, including filters over
built-in properties and `task.originalMarkdown`, but they still do not provide
a general "apply this field to all shown tasks" action.

### Obsidian CLI

The official Obsidian CLI can list tasks and update task status/toggle/done/todo
by task reference. Its `property:set` command targets note properties, not
individual task-line inline fields. It is useful for status edits or as another
way to inspect task references, but it does not solve arbitrary task-property
bulk setting.

### Frontmatter Bulk-Property Plugins

Multi-Properties is explicitly for adding, editing, or removing frontmatter
properties on multiple notes. Metadata Menu can manage note metadata and inline
fields from GUI contexts, but its bulk workflows are still note/table oriented
and do not provide the same headless, source-verified task-line rewrite flow.

These plugins are useful when the target property belongs to a whole note. They
are the wrong layer when the target is one task line inside a note.

### QuickAdd, Templater, or Custom Obsidian Script

An in-Obsidian script could run a Dataview query and rewrite matching lines via
Obsidian APIs. This is viable for a desktop-only one-off, but it duplicates what
a Bob-native script can do headlessly and needs the same stale-line, block-id,
and generated-page checks.

## Recommended Workflow

For a selector that can be expressed in Dataview:

```bash
bob dataview --format json --query '
TASK
WHERE contains(tags, "#task")
  AND !completed
  AND !scheduled
'
```

For a hand-curated list from Obsidian, normalize it before editing to one of:

```text
path/to/file.md<TAB>zero_based_line<TAB>expected task text
path/to/file.md:one_based_line<TAB>expected task text
path/to/file.md#^block-id<TAB>expected task text
```

Then run a purpose-built updater with this behavior:

1. Resolve `~/bob` and optionally run or wait for the normal `ob` sync path.
2. Check `git -C ~/bob status --short`; refuse dirty candidate files unless
   explicitly overridden.
3. Convert all candidates to vault-relative path plus zero-based line or block
   id.
4. Group candidates by Markdown file.
5. Read each file once, preserving line endings.
6. For every selected task:
   - verify the current line is still a Markdown task;
   - verify expected status/text or block id still matches;
   - if the field already exists, replace its value according to the chosen
     overwrite policy;
   - otherwise append `  [key:: value]` before any trailing `^block-id`;
   - preserve links, tags, existing inline fields, indentation, and checkbox
     status.
7. Print a unified diff and summary.
8. Write only with `--apply`.
9. Re-run a Dataview verification query showing the selected tasks now expose
   the expected field.

For a Bob-native command, a good interface would be:

```bash
bob task-prop set --property scheduled --value 2026-06-08 --query-file /tmp/tasks.dql
bob task-prop set --property scheduled --value 2026-06-08 --tasks-file /tmp/tasks.tsv --apply
```

Dry-run should be the default. Promotion into `bob-cli` is worth it if this will
recur; for a single cleanup, a small script using the same rules is sufficient.

## Safety Notes

- Do not use `sed -i` against matching text. It will not protect against stale
  line numbers, duplicate fields, code examples, nested non-task lines, or block
  ids.
- Do not depend on line numbers alone. Always verify current line content before
  rewriting.
- Prefer a block id or Tasks `[id:: ...]` if the same task set will be updated
  repeatedly.
- Decide idempotency up front: overwrite existing values, skip existing values,
  or error.
- For custom fields such as `snooze`, Dataview will index the inline field, but
  Tasks may not expose a typed property. Tasks custom filters can inspect
  `task.originalMarkdown` when needed.
- Treat `generated_from_zorg` as a migration warning. Do not automatically edit
  `/home/bryan/org/*.zo` just because a task row inherited that field.

## Sources

- Dataview - Adding Metadata:
  https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/
- Dataview - Metadata on Tasks and Lists:
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Dataview - TASK queries:
  https://blacksmithgu.github.io/obsidian-dataview/queries/query-types/#task
- Tasks - Dataview Format:
  https://github.com/obsidian-tasks-group/obsidian-tasks/blob/main/docs/Reference/Task%20Formats/Dataview%20Format.md
- Tasks - Create or edit Task:
  https://github.com/obsidian-tasks-group/obsidian-tasks/blob/main/docs/Editing/Create%20or%20edit%20Task.md
- Tasks - Task Properties:
  https://github.com/obsidian-tasks-group/obsidian-tasks/blob/main/docs/Scripting/Task%20Properties.md
- Obsidian CLI:
  https://obsidian.md/help/cli
- Metadata Menu:
  https://mdelobelle.github.io/metadatamenu/
- Multi-Properties:
  https://github.com/technohiker/obsidian-multi-properties
- Local: `docs/dataview.md`
- Local: `sdd/tales/202606/snooze_task_property.md`
- Local verification: `bob dataview` TASK JSON output and selected
  `~/bob`/`/home/bryan/org` line checks on 2026-06-04.
