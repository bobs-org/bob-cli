---
create_time: 2026-07-09
status: research
topic: De-duplicating the three near-identical Tasks queries in ~/bob/dash.md
---
# Research: De-duplicating the Tasks Queries in `dash.md`

## Question

`~/bob/dash.md` contains three ` ```tasks ` query blocks (WIP Tasks, NEXT
Tasks, READY Tasks) that are nearly identical — they share ~10 instruction
lines and differ only in a single status line. What are the available options
for factoring out the shared query text so it lives in one place, and which one
should we use?

## Answer

Use the **Tasks plugin's native "Presets" feature**. Define one preset (e.g.
`dash_common`) holding the ~10 shared instruction lines, then reduce each of the
three blocks to two lines: the block's unique status filter plus
`preset dash_common`.

This is the purpose-built, first-party mechanism for exactly this problem. It
requires no new plugins, keeps each block a real live `tasks` query (toolbar,
grouping, sorting all intact), and the vault is already on Tasks **v8.0.0**
(Presets shipped in 7.20.0) with presets already in active use. The shared text
ends up in exactly one place; each block keeps only what makes it distinct.

The alternatives (Global Query, Templater generation, dataviewjs) all work in a
narrow sense but are either too broad, only de-duplicate at authoring time, or
throw away Tasks-native behavior. Details below.

## Background: what is actually duplicated

All three blocks in `dash.md` are identical except for **one line** — the status
selector:

| Block        | Distinct line                    |
| ------------ | -------------------------------- |
| WIP Tasks    | `status.type is IN_PROGRESS`     |
| NEXT Tasks   | `status.name includes Next`      |
| READY Tasks  | `status.type is TODO`            |

Everything else is copy-pasted across all three (10 lines):

```text
folder does not include _templates
is not blocked
filter by function task.file.path !== query.file.path
filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), "day")
filter by function !task.tags.includes("#hide")
group by path
sort by function task.file.path
sort by function task.lineNumber
short mode
hide toolbar
```

So the ideal solution keeps the one distinct status line inside each block and
sources the 10 shared lines from a single definition. Note that instruction
order inside a `tasks` block does not matter to the plugin, so the status line
and the shared block can appear in either order.

Environment facts confirmed while researching:

- Installed Tasks plugin version: **8.0.0** (`.obsidian/plugins/obsidian-tasks-plugin/manifest.json`).
- Presets are already enabled and in use: `data.json` ships the stock presets
  (`this_file`, `this_folder`, `hide_date_fields`, `hide_everything`, …), and
  they already demonstrate **nesting** (`hide_everything` calls
  `preset hide_date_fields`) and **placeholders** (`this_file` uses
  `{{query.file.path}}`).
- `globalQuery` is currently empty (`""`).

## Options considered

### Option 1 — Tasks Presets (RECOMMENDED)

The Tasks plugin lets you save a named block of query instructions as a
**preset** and reuse it in any query. Introduced in **Tasks 7.20.0**; we run
8.0.0.

- **Define** in Settings → Tasks → *Presets* as a `name` → `instructions` pair.
  On disk this is just a string entry under the `presets` key of
  `.obsidian/plugins/obsidian-tasks-plugin/data.json` (multi-line values use
  `\n`).
- **Use** in a query two ways:
  - `preset <name>` — expands the named instructions inline (the normal case,
    for whole instruction lines).
  - `{{preset.<name>}}` — placeholder form, for when you need the fragment
    inside a Boolean combination (`AND`/`OR`/`NOT`) or a partial line.
- Presets can be **combined with other instructions** in the same block, can
  **nest** other presets, and can contain **placeholders** like
  `{{query.file.path}}`.
- Limitation: presets **cannot take parameters** — they are inserted verbatim.
  This is a non-issue here because the only thing that varies (the status line)
  simply stays in each block.

**Pros**

- Native, first-party, zero new dependencies; already enabled in this vault.
- Exact fit: shared text lives in one place, each block keeps only its status line.
- Each block stays a genuine `tasks` query — live filtering, grouping, sorting,
  toolbar all behave exactly as today.
- Reusable beyond `dash.md` if similar dashboards appear later.
- Editable headless: because presets are plain JSON in `data.json`, they can be
  managed via the `ob` workflow, not only the desktop GUI.

**Cons**

- The shared definition lives in plugin settings, not in `dash.md`, so it is
  slightly less discoverable from the note itself. (Mitigate with a `#`
  comment on the preset's first line documenting its purpose — Tasks supports
  this.)
- Vault-global namespace: the preset name is visible to every query in the
  vault (harmless, but pick a clear name like `dash_common`).

### Option 2 — Global Query

Tasks has a **Global Query** setting whose instructions are prepended to *every*
`tasks` block in the vault.

**Pros**

- Zero per-block syntax; shared filters apply automatically everywhere.

**Cons (disqualifying here)**

- Applies to **every query in the entire vault**, not just `dash.md`. These
  filters are dashboard-specific (`group by path`, `short mode`,
  `hide toolbar`, the "not this file" and "not #hide" filters) and are not
  wanted on unrelated queries elsewhere.
- Overriding it is only partial — per the docs, "it isn't always possible to
  override a filter set in the Global Query" (a query can opt out entirely with
  `ignore global query`, but cannot selectively drop one line).
- `globalQuery` is currently empty; hijacking it for one dashboard would be a
  surprising global side effect.

Verdict: wrong scope. Good for truly vault-wide defaults, not for three blocks
in one file.

### Option 3 — Templater generation

Use a Templater template that takes a status argument and emits a full `tasks`
block, invoked three times.

**Pros**

- Fully DRY at authoring time; can parameterize the status line.

**Cons**

- De-duplicates only at *generation* time — the rendered `dash.md` on disk still
  contains three fully-expanded blocks, so the file itself is not actually
  smaller/DRY unless kept as a template that must be re-run.
- Adds Templater indirection and a manual regeneration step for what is a static
  dashboard.
- More moving parts than Presets for no extra benefit here.

Verdict: over-engineered for this case.

### Option 4 — dataviewjs / Tasks API programmatic rendering

Render the three queries from a loop in a `dataviewjs` block (or via the Tasks
query API).

**Cons**

- Either rewrites the queries in Dataview's dialect (losing Tasks-specific
  status semantics, toolbar, and instruction set) or leans on non-obvious
  internal APIs.
- Highest complexity and lowest robustness of all options.

Verdict: not worth it.

### Option 5 — Do nothing

Keep the duplication.

- The only real cost today is the three-way manual edit whenever the shared
  filters change. Presets removes that cost cheaply, so there is little reason
  to accept the status quo.

## Recommended solution

Adopt **Option 1 (Presets)**.

### 1. Define the preset

Add a preset named `dash_common` (Settings → Tasks → Presets, or directly in
`data.json` under `presets`) with this value:

```text
# Shared filters/layout for the dash.md task lists
folder does not include _templates
is not blocked
filter by function task.file.path !== query.file.path
filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), "day")
filter by function !task.tags.includes("#hide")
group by path
sort by function task.file.path
sort by function task.lineNumber
short mode
hide toolbar
```

As a `data.json` entry (note the escaped newlines) it looks like:

```json
"dash_common": "# Shared filters/layout for the dash.md task lists\nfolder does not include _templates\nis not blocked\nfilter by function task.file.path !== query.file.path\nfilter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), \"day\")\nfilter by function !task.tags.includes(\"#hide\")\ngroup by path\nsort by function task.file.path\nsort by function task.lineNumber\nshort mode\nhide toolbar"
```

### 2. Rewrite the three blocks in `dash.md`

````markdown
### WIP Tasks

```tasks
status.type is IN_PROGRESS
preset dash_common
```

### NEXT Tasks

```tasks
status.name includes Next
preset dash_common
```

### READY Tasks

```tasks
status.type is TODO
preset dash_common
```
````

Each block drops from ~12 lines to 2, and the shared logic now has a single home.

### 3. Verify

Open `dash.md` in Obsidian (or reload the vault) and confirm all three lists
render identically to before. If Tasks reports an unknown-preset error, the
preset name/definition didn't save — re-check the `presets` entry in
`data.json` or the Presets settings pane.

## Notes / caveats

- **Editing `data.json` directly:** if done outside the GUI (e.g. headless via
  `ob`), make sure Obsidian isn't simultaneously writing the file, and keep the
  JSON valid. Using the Settings → Presets UI is the safest path when a GUI is
  available.
- **Documentation comment:** the leading `# …` line inside the preset is a Tasks
  comment; it documents intent and is ignored during execution.
- **Placeholder form not needed here:** `preset dash_common` (statement form) is
  correct because we're inserting whole instruction lines. Reserve
  `{{preset.dash_common}}` for cases where a fragment must sit inside a Boolean
  expression.
- **Optional further factoring:** the shared block's `hide toolbar` overlaps with
  the stock `hide_query_elements` preset; if you later want to hide more toolbar
  elements you could nest presets. Not necessary to match current behavior.

## Sources

- [Presets — Tasks User Guide](https://publish.obsidian.md/tasks/Queries/Presets)
- [Global Query — Tasks User Guide](https://publish.obsidian.md/tasks/Queries/Global+Query)
- [About Queries — Tasks User Guide](https://publish.obsidian.md/tasks/Queries/About+Queries)
- [Query Language Syntax — DeepWiki (obsidian-tasks-group/obsidian-tasks)](https://deepwiki.com/obsidian-tasks-group/obsidian-tasks/3.1-query-language-syntax)
- Local: `~/bob/.obsidian/plugins/obsidian-tasks-plugin/manifest.json` (v8.0.0) and `data.json` (existing presets, empty `globalQuery`)
