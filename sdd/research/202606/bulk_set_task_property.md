---
create_time: 2026-06-04
status: research
topic: Setting the same property on a list of Obsidian tasks that live in different files, in bulk
---
# Research: Bulk-Setting a Property on Tasks Across Many Files

## Answer

There is no single Obsidian button for "add this property to these specific task
lines in these different files." The popular bulk-property plugins
(Multi-Properties, Metadata Menu bulk edit) operate on **file-level
frontmatter**, not on individual task list items, so they do not solve the
stated problem directly.

For a task-line property the correct unit is a **Dataview bracket inline field**
written on the task line itself:

```md
- [ ] Send David the deadline email [due:: 2026-06-10]
```

The best fit for this repo is a small two-step, scriptable pipeline rather than a
GUI plugin:

1. **Discover** the exact task list with `bob dataview --format json` using a
   `TASK ... WHERE ...` query. The JSON already gives you, per task, the file
   `path`, the 0-indexed `line`, the task `text`, the `blockId`, and — crucially
   for this vault — the originating `zorg_source` / `zorg_source_abs`.
2. **Mutate** each matched task line by appending the inline field, idempotently.

The decisive caveat is below: most Bob tasks are **generated from `.zo` source
files**, so the durable edit must land in the `.zo` source, not the generated
`.md`.

## Decisive Local Constraint: Most Bob Tasks Are Zorg-Generated

Verified in `~/bob` on 2026-06-04 via `bob dataview --format json
--query 'TASK WHERE !completed LIMIT 2'`. Every returned task carried:

- `"generated_from_zorg": true`
- `"zorg_source": "2023/20230729.zo"`,
  `"zorg_source_abs": "/home/bryan/org/2023/20230729.zo"`
- `"zorg_converter": "convert_zorg_core.py"`

Cross-checking the files confirmed the generation direction:

- `~/bob/2023/20230729.md` line 28 (1-indexed) is `- [g] Daily log`.
- `~/org/2023/20230729.zo` line 19 is the identical `- [g] Daily log`.

So the `.md` is a build artifact of the `.zo` source. **Editing the generated
`.md` task line would be overwritten the next time the converter runs.** For any
zorg-generated task, the property must be written into the `.zo` source line and
the markdown regenerated.

This splits the problem into two regimes, and the JSON's `generated_from_zorg`
flag tells you which regime each task is in:

| Task origin | Source of truth to edit | How to locate the line |
| --- | --- | --- |
| `generated_from_zorg: true` | the `.zo` file (`zorg_source_abs`) | match by task `text` inside the `.zo` file; the `.md` `line` does **not** apply to the `.zo` |
| `generated_from_zorg: false` / absent | the `.md` file (`path`) | use the `line` field directly (0-indexed) |

Confirmed indexing detail: Dataview's `line` is **0-indexed** — reported
`line: 27` corresponds to 1-indexed markdown line 28. Any mutation script must
account for this off-by-one.

A second open question for the zorg regime: whether `convert_zorg_core.py`
even preserves arbitrary `[key:: value]` inline fields on round-trip, and what
the `.zo` annotation syntax for a per-task property is. That must be checked
against the zorg converter before committing to a `.zo`-editing approach.

## The Two Things People Call "a property"

Disambiguating this up front avoids picking the wrong tool:

- **Frontmatter / YAML property** — applies to the whole *file*, cannot target a
  single task. This is what the bulk GUI plugins edit.
- **Task-line inline field** — `[key:: value]` bracket syntax written directly on
  the `- [ ] ...` line. This is the only way to attach metadata to one specific
  task. Per the Dataview docs, on a list item you must use the **bracket** form
  (`[due:: 2026-06-10]`) because the line already holds other text; the bare
  `key:: value` form is only valid when it is the whole line. A **parenthesis**
  variant `(due:: 2026-06-10)` hides the key in Reader view.

Since the request is "the same property on a list of tasks," the target is the
task-line inline field, regime-routed per the table above.

## Option Survey

### A. Bob-native scripted pipeline (recommended)

`bob dataview` already exposes exactly the discovery half of the problem:

```bash
bob dataview --format json --query 'TASK WHERE <selector>'
```

The JSON `result.values[]` gives `path`, `line`, `text`, `blockId`,
`generated_from_zorg`, and `zorg_source_abs` per task — everything a mutation
step needs to find and edit each line safely. A companion mutation step (a new
`bob` subcommand, e.g. `bob set-task-prop`, or a one-off script) then:

- routes each task to `.zo` or `.md` per `generated_from_zorg`;
- for `.md` tasks: rewrites the 0-indexed `line`, inserting `[key:: value]`
  before any trailing block id (`^abc123`);
- for `.zo` tasks: locates the matching line in `zorg_source_abs` by task text
  and inserts the property there, then triggers regeneration;
- is **idempotent**: if `key::` is already present, update its value rather than
  appending a duplicate;
- ideally previews a dry-run diff before writing.

Pros: matches the repo's existing native-command pattern, handles the zorg
regime correctly, no new Obsidian plugin dependency, works headlessly/in cron.
Cons: requires building the mutation step; must verify `.zo` round-trip safety.

### B. Metadata Menu plugin

Adds context-menu and command-palette field editing, and advertises bulk field
edits "in multiple notes at once from tableviews and mdm code blocks." Its
strength is structured frontmatter / file fields. It is **not** designed to set
an inline field on a specific selected set of task *lines* across files, and it
is GUI/desktop-bound. Useful if the property is actually a file-level property;
a poor fit for per-task metadata and for headless automation.

### C. Multi-Properties plugin

Lets you add/edit/remove properties on many notes selected by folder, file
explorer, or search results. Explicitly **frontmatter-only** — it cannot target
individual task lines. Wrong granularity for this request.

### D. Dataview-to-Properties / Better Inline Fields plugins

`Dataview Properties` copies inline fields *into* frontmatter and keeps them
synced; `Better Inline Fields` improves editing/autocomplete of inline fields.
Both are about an existing field's lifecycle, not bulk-applying a new field to a
chosen set of tasks. Adjacent, not a solution.

### E. Tasks plugin

The Obsidian Tasks plugin has no built-in bulk operation to add a field to many
tasks across files — community requests for this remain open feature requests.
It also has its own emoji/`due`-style metadata vocabulary distinct from Dataview
inline fields, so mixing the two needs care. Not a bulk solution here.

### F. QuickAdd / Templater scripting (in-Obsidian)

Can programmatically read a Dataview query and rewrite lines via the editor API.
Viable for a desktop-only, interactive workflow, but duplicates what a `bob`
subcommand would do, stays GUI-bound, and is **zorg-unaware** — it would edit the
generated `.md` and get clobbered. Not recommended for this vault.

## Recommendation

1. Treat the property as a **task-line Dataview inline field** (`[key:: value]`),
   not frontmatter — so rule out the bulk frontmatter plugins (B, C).
2. Use `bob dataview --format json --query 'TASK WHERE ...'` as the **selector**
   to produce the exact task list with file/line/text/zorg metadata.
3. Add a **zorg-aware mutation step** that routes each task to its `.zo` source
   (most tasks) or its `.md` (hand-authored tasks), inserts the field
   idempotently before any block id, and supports a dry-run diff.
4. Before building the `.zo` path, **verify with `convert_zorg_core.py`** that an
   inline `[key:: value]` survives conversion and learn the `.zo`-native way to
   express a per-task property — it may be cleaner to set the property in `.zo`
   syntax than to inject Dataview bracket syntax.

For a genuinely one-off, desktop-only, **non-zorg** set of tasks, Metadata Menu
or a Templater script are acceptable shortcuts — but for anything in `~/bob`'s
generated notes, only the source-aware pipeline edits the right file.

## Open Questions to Resolve Before Implementation

- How will the task list be specified — a reusable DQL `WHERE` selector, or a
  hand-curated list of files/block-ids? A selector makes this repeatable.
- Does `convert_zorg_core.py` preserve `[key:: value]` on a task line, and what
  is the `.zo`-native per-task property syntax?
- Should the property also be queryable as-is, or eventually promoted into
  frontmatter (which would bring Dataview-to-Properties back into scope)?
- Idempotency policy: on re-run, overwrite an existing value, skip, or error?

## Sources

- Dataview — Adding Metadata (inline field syntax):
  https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/
- Dataview — Metadata on Tasks and Lists (bracket syntax requirement):
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Metadata Menu plugin (bulk field edit scope):
  https://mdelobelle.github.io/metadatamenu/
- Bulk edit properties with Metadata Menu (forum):
  https://forum.obsidian.md/t/bulk-edit-properties-with-metadata-menu-0-8-0-beta/76702
- Multi-Properties plugin (frontmatter-only bulk):
  https://github.com/technohiker/obsidian-multi-properties
- Dataview to Properties plugin:
  https://www.obsidianstats.com/plugins/dataview-properties
- Better Inline Fields plugin:
  https://www.obsidianstats.com/plugins/better-inline-fields
- Obsidian Tasks plugin:
  https://github.com/obsidian-tasks-group/obsidian-tasks
- Update properties of files from search results in bulk (forum):
  https://forum.obsidian.md/t/update-properties-of-files-from-search-results-in-bulk/72510
- Local verification: `bob dataview --format json` TASK output; `~/bob/2023/20230729.md` vs `~/org/2023/20230729.zo` (2026-06-04)
