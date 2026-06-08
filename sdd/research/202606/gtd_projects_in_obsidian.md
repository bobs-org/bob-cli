---
create_time: 2026-06-08
status: research
topic: Representing GTD projects in Bryan's Obsidian vault
---
# Research: GTD Projects in Obsidian

## Question

How should Bryan represent projects, in the GTD sense, in the `~/bob`
Obsidian vault?

## Short Answer

Use a first-class project note for each current GTD project, with stable
frontmatter for inventory/review and normal Markdown body content for outcome,
support material, and thinking. Keep next actions as normal `#task` checkbox
lines using the existing Tasks plugin Dataview format. Link tasks to project
notes with `[project:: [[project-note]]]` when they live outside the project
note.

Do not treat the generated `#project/foo` marker pages as canonical GTD
projects. They are useful legacy indexes from `+foo` markers, but the vault
already shows that many of those markers are labels, workstreams, topics, or
historical buckets rather than current GTD outcomes.

## Local Context Checked

Checked on 2026-06-08:

- Project memory says `~/bob/` is the active Obsidian vault, `ob` is used for
  Obsidian Headless Sync, and new Markdown notes under `~/bob/` should include
  a `parent` frontmatter field.
- Prior research says the vault uses Obsidian Sync, Dataview, Tasks, Templater,
  QuickAdd, custom plugins, and native Bases. Tasks is configured with
  `globalFilter: "#task"` and `taskFormat: "dataview"`.
- `job.md` and `obsidian.md` are the only non-generated notes found through
  `FROM #project WHERE !contains(file.path, "_generated")`; both have
  `type: project`, `status: active`, `area`, `id`, and `tags: [project]`.
- `job.md` is the strongest existing precedent: it is a project note with
  frontmatter plus active `#task` checkbox lines using Dataview-style fields
  such as `[completion:: 2026-05-31]`, `[scheduled:: 2026-06-12]`,
  `[dependsOn:: ...]`, and `[id:: ...]`.
- The legacy `prj_*.md` notes still exist and are largely migrated `zorg`
  project/action list files. Examples include `prj_work.md`, `prj_gtd.md`,
  `prj_zorg.md`, `prj_yserve.md`, and `prj_mcr_cats.md`.
- `_generated/tag_pages/project.md` is a generated marker index. It says
  markers are normalized only in generated index pages and that source note
  text remains authoritative. It contains many `#project/foo` pages generated
  from old `+foo` markers, for example `+rap`, `+gbd`, `+neovim`, `+zorg`,
  `+yserve`, and many narrower feature or label-like markers.
- `refs.base` shows a good local Bases pattern: native `.base` files can be
  hand-edited YAML with filters, formulas, display names, multiple views,
  grouping, and sorting.

## GTD Requirements

GTD uses a broader definition of "project" than most project-management tools.
The useful constraints for the vault are:

- A project is an outcome you are committed to finish within roughly a year
  that requires more than one action.
- The project list is an inventory of outcomes, not a task list.
- A current project should have at least one current next action, waiting-for,
  or calendar item.
- Future/dependent actions should not be forced onto next-action lists before
  they are actionable; they belong in project support material.
- Projects should be named by the outcome that will be true when they are done.
- The project inventory and project support should be reviewed during the
  Weekly Review.

These constraints argue for one durable "project record" per commitment, plus
queries that show whether each project has live action/wait/calendar coverage.
They do not require a full PM system, a note per task, a folder per project, or
a Kanban board per project.

## Obsidian Constraints

### Properties Are the Right Layer for Inventory

Obsidian properties are structured note metadata stored as YAML frontmatter.
They support simple machine-readable fields such as text, list, number,
checkbox, date, date-time, and tags. Obsidian explicitly treats properties as
small atomic data, not rich Markdown. That fits GTD project inventory fields:
status, area, priority, review cadence, and stable identifiers belong in
frontmatter; project thinking and support material belong in the note body.

### Bases Are Good for Project Lists, Not Task Queries

Native Bases create database-like views over notes and their properties. They
can filter, sort, group, display multiple views, and define formulas in `.base`
YAML. That makes Bases a strong fit for the GTD Projects List:

- active projects by area;
- waiting/on-hold projects;
- someday/incubated projects;
- recently closed projects;
- projects needing review.

But Bases are note-oriented. They should not replace Tasks/Dataview for
task-line dashboards. Use Bases for the project inventory and use Tasks or
Dataview for action lists.

### Tags Are Useful, But Not Canonical Here

Obsidian nested tags are convenient for search and Bases filters, but the vault
already has a generated `#project/...` namespace created from legacy `+foo`
markers. Because `#project` matches nested tags, using `file.hasTag("project")`
alone would mix canonical GTD project notes with generated marker pages.

For canonical GTD projects, prefer `type: project` plus explicit fields. Tags
can remain useful for human search, but they should not be the source of truth.

### Existing Task Format Should Be Preserved

The vault already uses the Tasks plugin with Dataview task format and a
`#task` global filter. Dataview supports inline fields on task/list lines, and
Tasks supports Dataview-style fields such as `[priority:: high]`,
`[scheduled:: YYYY-MM-DD]`, `[due:: YYYY-MM-DD]`, `[created:: YYYY-MM-DD]`,
and `[completion:: YYYY-MM-DD]`.

Therefore project linkage should use the same inline-field shape:

```markdown
- [ ] #task Email Pat the signed form  [project:: [[job]]]  [scheduled:: 2026-06-12]
```

For tasks inside the project note itself, the note path already provides the
project context. For tasks captured in daily notes, meeting notes, reference
notes, or inbox notes, add `[project:: [[project-note]]]` when the project
association matters.

## Options Considered

### Option 1: Keep Only `prj_*.md` List Notes

This is the lowest migration cost because the vault already has many
`prj_*.md` files and legacy project buckets.

Problem: those files are closer to migrated action-list/source-material buckets
than a clean GTD Projects List. They mix project identifiers, action lines,
references, old statuses, and generated-from-zorg structure. They are useful
history, but they do not give a clear inventory of current outcomes.

Verdict: keep as legacy support material, not as the future canonical model.

### Option 2: Use Only `#project/foo` Tags

This would align with the generated marker index and old `+foo` project tokens.

Problem: the generated marker set is too broad. It contains real projects,
subprojects, labels, features, topics, books, routines, historical buckets, and
workstream tags. GTD projects need a commitment boundary and outcome statement,
not just a marker.

Verdict: useful as a bridge/index, not canonical.

### Option 3: Use a Project-Management Plugin

Several plugin directions exist:

- Project Manager stores projects and tasks as Markdown files with YAML
  frontmatter and offers table, Gantt, Kanban, dependencies, milestones,
  scheduling, time, custom fields, and bulk actions.
- TaskNotes makes each task a Markdown note with YAML frontmatter and powers
  views through Bases.
- Tag Project manages tasks anywhere through tags, frontmatter, workflows, and
  Dataview.
- The older Obsidian Projects plugin could create table/board/calendar/gallery
  views, but its maintainer announced discontinuation in May 2025.

These are credible tools, but they solve a heavier problem than GTD project
inventory. The current vault already has a working Tasks/Dataview model,
thousands of legacy task lines, and Bob CLI tooling around Dataview. Moving to
note-per-task or plugin-specific project files would create a second task data
model and a migration burden.

Verdict: do not adopt a PM plugin for this problem unless Bryan later wants
Gantt/Kanban/team-assignment features badly enough to accept a new data model.

### Option 4: First-Class Project Notes Plus Existing Tasks

Create or maintain one canonical note for each GTD project. Use the note as
the inventory record and support-material container. Keep actions as normal
Tasks/Dataview `#task` lines.

This matches:

- GTD's distinction between project outcomes and next actions;
- the existing `job.md` and `obsidian.md` pattern;
- Obsidian properties and Bases;
- the existing Tasks global filter and Dataview task metadata;
- the existing generated project-marker pages, because they can remain
  compatibility indexes instead of being promoted to canonical records.

Verdict: best fit.

## Proposed Project Note Schema

Use this as the canonical GTD project-note shape:

```markdown
---
type: project
status: active
area: work
priority: P2
id: yserve
parent: "[[work]]"
aliases:
  - "YServe outcome name"
tags:
  - project
legacy_markers:
  - "+yserve"
legacy_project_tags:
  - "#project/yserve"
review_cadence: weekly
next_review: 2026-06-15
done_tasks: "[[done/yserve_done]]"
---
# YServe outcome name

## Outcome

One sentence that says what will be true when this is complete.

## Current Actions

- [ ] #task Concrete visible next action  [scheduled:: 2026-06-12]

## Waiting For

- [ ] #task Waiting for Pat to reply  [context:: waiting]  [project:: [[yserve]]]

## Project Support

Notes, links, constraints, brainstormed future actions, and parked sequential
steps. Future dependent actions should live here until they are actionable.

## Completion Criteria

- ...
```

Notes:

- Keep `type: project` as the machine-readable selector for canonical project
  notes.
- Keep `status` small and enumerable. Suggested values:
  `active`, `waiting`, `on_hold`, `someday`, `done`, `dropped`.
- Keep `area` as a stable link or short value that maps to a higher-horizon
  area of responsibility.
- Keep `parent` because vault memory requires it for new notes under `~/bob`.
- Keep `legacy_markers` only when a project note corresponds to legacy `+foo`
  markers. Do not invent legacy markers for new projects.
- Use body sections for rich details. Do not put long project plans into YAML.

## Suggested Dashboards

### `projects.base`

Create a native Base for the project inventory. It can follow the same style as
`refs.base`:

```yaml
filters:
  and:
    - type == "project"
formulas:
  title_link: if(note.aliases, file.asLink(note.aliases[0]), file.asLink())
properties:
  formula.title_link:
    displayName: Project
  status:
    displayName: Status
  area:
    displayName: Area
  priority:
    displayName: Priority
  next_review:
    displayName: Review
views:
  - type: table
    name: Active
    filters:
      and:
        - status == "active"
    groupBy:
      property: area
      direction: ASC
    order:
      - formula.title_link
      - priority
      - next_review
      - file.mtime
    sort:
      - property: priority
        direction: ASC
      - property: next_review
        direction: ASC
  - type: table
    name: Waiting / On Hold
    filters:
      or:
        - status == "waiting"
        - status == "on_hold"
    groupBy:
      property: area
      direction: ASC
  - type: table
    name: Someday
    filters:
      and:
        - status == "someday"
  - type: table
    name: Closed
    filters:
      or:
        - status == "done"
        - status == "dropped"
```

This is intentionally a project inventory dashboard, not a task board.

### Project-Local Task Query

In each project note, use Dataview/Tasks to show incomplete actions from the
project note and actions elsewhere that explicitly link back to it:

```dataview
TASK
FROM ""
WHERE contains(tags, "#task")
  AND !completed
  AND (
    file.link = this.file.link
    OR project = this.file.link
    OR contains(project, this.file.link)
  )
SORT scheduled ASC, due ASC, priority DESC
```

Treat this as a template candidate, not a final tested query for every edge
case. The important rule is the data shape: task-line metadata should point to
the project note when the task is not already in the project note.

### Weekly Review Query

For Weekly Review, the useful question is not "show me all project tasks." It
is "which current projects lack a visible current action, waiting-for, or
calendar/tickler item?" Native Dataview may need a small DataviewJS query or a
Bob CLI helper to answer that perfectly, because it has to join project-note
properties against task-line metadata.

Start manually with `projects.base`. Later, automate the review gap check:

- select project notes where `type: project` and `status` is `active`;
- find incomplete `#task` lines either inside that note or with
  `[project:: [[that note]]]`;
- count tasks that are scheduled, due, waiting, or otherwise actionable;
- report active projects with zero coverage.

## Migration Plan

1. Do not bulk-convert every `+foo` marker into a GTD project note.
2. Treat `job.md` and `obsidian.md` as the seed pattern.
3. During Weekly Review, create canonical project notes only for commitments
   that are truly current GTD projects.
4. When a legacy `+foo` marker maps to the current project, add it to
   `legacy_markers` on the project note.
5. When touching a current task outside the project note, add
   `[project:: [[project-note]]]`.
6. Keep generated `_generated/tag_pages/project/*.md` pages as read-only
   compatibility indexes.
7. Add `projects.base` once there are enough canonical project notes for the
   dashboard to matter.
8. Add automation later for "active projects with no current action" if the
   manual review starts to feel tedious.

## Sources

- GTD, "Managing projects with GTD":
  https://gettingthingsdone.com/2017/05/managing-projects-with-gtd/
- GTD, "The Elusive Inventory of Your Projects":
  https://gettingthingsdone.com/wp-content/uploads/2014/10/Project_Inventory.pdf
- Obsidian Help, "Properties":
  https://obsidian.md/help/properties
- Obsidian Help, "Create a base":
  https://obsidian.md/help/bases/create-base
- Obsidian Help, "Views":
  https://obsidian.md/help/bases/views
- Obsidian Help, "Bases syntax":
  https://obsidian.md/help/bases/syntax
- Obsidian Help, "Tags":
  https://obsidian.md/help/tags
- Dataview, "Adding Metadata":
  https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/
- Dataview, "Metadata on Tasks and Lists":
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Tasks User Guide, "About Task Formats":
  https://publish.obsidian.md/tasks/Reference/Task%20Formats/About%20Task%20Formats
- Obsidian community plugin page, "Project Manager":
  https://community.obsidian.md/plugins/project-manager
- TaskNotes GitHub README:
  https://github.com/callumalpass/tasknotes
- Obsidian Projects GitHub README:
  https://github.com/obsmd-projects/obsidian-projects
- Obsidian community plugin page, "Tag Project":
  https://community.obsidian.md/plugins/tag-project-odaimoko

## Recommended Solution

Adopt first-class GTD project notes as the canonical project representation:
one note per committed multi-step outcome, with `type: project`, `status`,
`area`, `priority`, `id`, `parent`, and optional legacy marker fields in
frontmatter. Use the note body for the outcome statement, current actions,
waiting-for items, project support, and completion criteria.

Keep next actions as the existing inline `#task` checkbox lines using Tasks'
Dataview format. For actions outside the project note, add
`[project:: [[project-note]]]`. For actions inside the project note, rely on
the source note unless a cross-note dashboard needs explicit linkage.

Use native Bases for the Projects List and Weekly Review inventory. Keep
Dataview/Tasks for task-line dashboards. Leave generated `#project/foo` marker
pages and legacy `prj_*.md` notes in place as compatibility/reference material;
promote only true current GTD commitments into canonical project notes as they
surface in review.
