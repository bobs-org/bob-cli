---
create_time: 2026-06-08
status: research
topic: Representing GTD projects in Bryan's Obsidian vault
---
# Research: Representing GTD Projects in the Bob Obsidian Vault

## Question

How should Bryan represent projects, in the GTD sense, in the `~/bob` Obsidian
vault?

## Executive Answer

Use first-class Obsidian project notes as the canonical GTD Projects List: one
note for each committed multi-step outcome, with small structured properties
for inventory and review, plus normal Markdown body sections for outcome
thinking, project support, and current actions.

Keep action tracking in the current task model: inline Markdown checkbox tasks
with `#task` and Dataview-style fields. When a task lives outside its project
note, link it back with `[project:: [[project-note]]]`. Keep legacy `prj_*`
notes and generated `#project/...` pages as reference/compatibility material,
but do not treat every legacy `+marker` as a current GTD project.

The practical target is:

- a queryable project inventory built from notes with `type: project`;
- a native `projects.base` dashboard for active, waiting, someday, paused, and
  done projects;
- Dataview/Tasks dashboards for next actions and waiting-for items;
- a weekly review check for active projects with no current action, waiting-for,
  or calendar/tickler item.

## Verification Performed

Checked on 2026-06-08:

- Read both prior agent transcripts:
  `~/.sase/chats/202606/bob_cli-ace_run-260608_112707.md` and
  `~/.sase/chats/202606/bob_cli-ace_run-260608_112719.md`.
- Read the two agent-created research drafts:
  `sdd/research/202606/gtd_projects_in_obsidian.md` and
  `sdd/research/202606/gtd_projects_obsidian_representation.md`.
- Read audited Obsidian memory with `sase memory read long/obsidian.md`.
- Verified local vault evidence with targeted reads and `bob dataview`.
- Checked current official docs for GTD, Obsidian Properties/Bases, Dataview
  task metadata, and the Obsidian Tasks plugin.

Important conflict resolved:

- The second draft centered the recommendation on changing a Zorg-to-Obsidian
  converter and generating project stubs for all legacy markers.
- The audited Obsidian memory says `~/bob/` is Bryan's active Obsidian vault and
  the previous Zorg migration is historical context. However, current vault
  files still contain `generated_from_zorg` metadata, and `_meta/Bob Home.md`
  plus `_meta/Active Workflow Pilot.md` still describe Bob as a generated mirror.
- Therefore this research should not prescribe a new Zorg-centered workflow.
  It should recommend an Obsidian-native project model. If implementation later
  touches generated notes, the current write path must be confirmed first.

## Local Findings

### Current Canonical Project Pattern Is Sparse

Only two non-generated project-tagged notes surfaced through Dataview:
`job.md` and `obsidian.md`. Targeted search found only those two notes with
`type: project` frontmatter.

`job.md` is the strongest local precedent:

- `type: project`
- `status: active`
- `area: job`
- `priority: P1`
- `tags: [job, project]`
- `id: job`
- inline `#task` checkbox lines with Dataview-format fields such as
  `[completion::]`, `[scheduled::]`, `[dependsOn::]`, and `[id::]`.

`obsidian.md` has the same basic project-note direction, with `type: project`,
`status: active`, `area: dev`, `parent`, `aliases`, `id`, `tags`, and
`done_tasks`.

This argues for promoting the existing `type: project` pattern, not inventing a
separate project-management data model.

### Legacy Project Material Is Broad And Mixed

The vault still has 14 root-level `prj_*` hub notes:

- `prj_bbxo`
- `prj_bs_allow`
- `prj_gbd`
- `prj_gtd`
- `prj_ilar`
- `prj_mcr_cats`
- `prj_pa_trouble`
- `prj_pd_local`
- `prj_plex`
- `prj_protect`
- `prj_tick`
- `prj_work`
- `prj_yserve`
- `prj_zorg`

The vault also has 445 generated project-marker pages under
`_generated/tag_pages/project/`. `_generated/tag_pages/project.md` says these
are normalized marker index pages and that source note text remains
authoritative. The marker set includes real projects, subprojects, labels,
features, routines, old workstreams, reading buckets, and household chores.

That breadth is useful for search and migration history, but it is too broad to
serve as a GTD Projects List. GTD projects are commitments with desired
outcomes, not just tags or historical buckets.

### Task Infrastructure Already Points In The Right Direction

The Tasks plugin settings currently use:

- `globalFilter: "#task"`
- `taskFormat: "dataview"`
- custom statuses for in progress, blocked, and canceled.

Text search found 451 Markdown checkbox task lines containing `#task`. The
native `bob dataview` TASK surface can see Markdown tasks, but the exact task
queries for project joins should be tested before depending on them. Direct
local examples in `job.md` confirm the desired syntax is already in use.

The current task model should be preserved. Moving to note-per-task, or
mass-converting every legacy org-style line into a Tasks plugin task, is a
separate migration decision and is not necessary for representing GTD projects.

### Bases Are A Good Fit For Project Inventory

`refs.base` is already a real local Bases dashboard with filters, formulas,
multiple views, grouping, ordering, and sorting. Obsidian Bases are note-oriented
database views, so they are a strong fit for a Projects List built from project
note properties. They are not a replacement for task-line queries.

## GTD Requirements

Official GTD guidance defines a project broadly: any outcome requiring more
than one action step that can be finished in roughly the next 12 months. GTD
also says:

- the Projects List is a current inventory or table of contents of outcomes;
- most people have many current projects, not just a handful;
- a current project should have at least one next action, waiting-for, or
  calendar item;
- future dependent actions belong in project support until actionable;
- project names should describe the outcome that will be true when done;
- the Projects List and project plans are reviewed in the Weekly Review.

For Bob, that means the project representation needs two layers:

- a durable project record for the commitment and review metadata;
- task/action records that can be queried separately and linked back.

It does not require Gantt charts, Kanban boards, folders per project, or a task
note for every action.

## Obsidian Requirements

Obsidian Properties are the right place for small, atomic project metadata:
`type`, `status`, `area`, `priority`, `parent`, `next_review`, and similar
fields. Obsidian explicitly treats properties as structured metadata, while rich
Markdown belongs in the note body.

Dataview supports frontmatter fields and task/list inline fields. For fields on
a specific task line, bracket inline syntax is the right shape:

```markdown
- [ ] #task Email Pat the signed form [project:: [[job]]] [scheduled:: 2026-06-12]
```

Obsidian Bases should hold the project inventory views. Tasks and Dataview
should hold action dashboards.

## Options Considered

### Option 1: Keep Only `prj_*` And Generated Marker Pages

This is low effort, but it does not solve the GTD problem. The `prj_*` hubs are
useful planning/reference notes, and generated `#project/...` pages are useful
indexes, but neither gives a clean queryable inventory of current commitments
with outcome, status, area, and next-action coverage.

Verdict: keep as support/reference, not canonical.

### Option 2: Promote Every Legacy `+marker`

The second draft recommended thin generated records for all active markers. That
would make everything queryable, but it over-promotes a mixed marker namespace.
Many markers are tags, features, chores, reading buckets, or old workstreams.

Verdict: useful later only for confirmed active commitments. Do not bulk-promote
all marker pages into GTD projects.

### Option 3: Adopt A Project-Management Plugin

Project-management plugins can add tables, Kanban, Gantt, milestones, or
note-per-task models. That is heavier than the request requires and would create
a second task/project model alongside existing Tasks, Dataview, Bases, and Bob
CLI tooling.

Verdict: do not add a project-management plugin for this. Reconsider only if
Bryan later wants PM-specific views badly enough to accept a migration.

### Option 4: First-Class Project Notes Plus Existing Tasks

One canonical project note per current GTD outcome, with project properties for
inventory and task lines linked by note context or `[project:: [[...]]]`.

This matches GTD, the existing `job.md` / `obsidian.md` precedent, Obsidian
Properties, Bases, Dataview inline fields, and the Tasks plugin configuration.

Verdict: best fit.

## Proposed Project Note Shape

Use this as the canonical project note template:

```markdown
---
type: project
status: active
area: dev
priority: P2
parent: "[[dev]]"
aliases:
  - "Readable project name"
project_key: obsidian
legacy_markers:
  - "+obsidian"
next_review: 2026-06-15
done_tasks: "[[done/obsidian_done]]"
tags:
  - project
---
# Readable project outcome

## Outcome

One sentence describing what will be true when this is complete.

## Current Actions

- [ ] #task Concrete next action [scheduled:: 2026-06-12]

## Waiting For

- [ ] #task Waiting for Pat to reply [project:: [[this-project]]] [waiting_on:: Pat]

## Project Support

Notes, constraints, links, brainstormed future actions, and inactive sequential
steps.

## Completion Criteria

- ...
```

Field notes:

- `type: project` is the canonical selector.
- `status` should stay enumerable: `active`, `waiting`, `paused`, `someday`,
  `done`, `dropped`.
- `area` maps the project to a horizon-2 area such as `dev`, `work`, `job`,
  `own`, `body`, `mind`, `love`, or `gtd`.
- `outcome` can be a body section rather than a property if Bryan wants richer
  wording, but the Projects List should display either an `outcome` property or
  the note title/alias as an outcome sentence.
- `project_key` is optional. Use it only when a project needs to join to legacy
  `+marker` material. Do not rely on the existing `id` field for project joins;
  it already exists locally, but it also triggered parser friction in prior
  Dataview experiments.
- `legacy_markers` is optional and should only record real historical markers.
- `parent` should be present for new notes in `~/bob`.

## Suggested Views

### `projects.base`

Create a native Base for project inventory:

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
  - type: table
    name: Waiting
    filters:
      and:
        - status == "waiting"
  - type: table
    name: Paused
    filters:
      and:
        - status == "paused"
  - type: table
    name: Someday
    filters:
      and:
        - status == "someday"
  - type: table
    name: Done
    filters:
      or:
        - status == "done"
        - status == "dropped"
```

The exact Base syntax should be tested in Obsidian before committing it, but
the structure matches the already-working `refs.base` style.

### Project-Local Task Query

Inside each project note, use a task query scoped to the current note plus
tasks elsewhere that link back:

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
SORT scheduled ASC, due ASC
```

Treat this as a query sketch. The data contract matters more than this exact
DQL: outside tasks must have `[project:: [[project-note]]]`.

### Weekly Review Coverage

The central review question should be:

Which active projects have no visible current action, waiting-for, or
calendar/tickler item?

Start manually with `projects.base`. Once the project-note pattern is stable,
add a DataviewJS query or `bob` helper that:

- selects notes where `type: project` and `status: active`;
- finds incomplete `#task` lines in the project note;
- finds incomplete `#task` lines elsewhere with `[project:: [[that note]]]`;
- counts tasks with scheduled/due/current/waiting markers;
- reports projects with zero coverage.

## Sources

Local sources:

- Audited memory: `sase memory read long/obsidian.md`
- `~/bob/job.md`
- `~/bob/obsidian.md`
- `~/bob/prj.md`
- `~/bob/prj_*.md`
- `~/bob/projects.md`
- `~/bob/project_checklists.md`
- `~/bob/_generated/tag_pages/project.md`
- `~/bob/_meta/Bob Home.md`
- `~/bob/_meta/Active Workflow Pilot.md`
- `~/bob/refs.base`
- `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json`
- `bob dataview` checks for non-generated `#project` notes and task visibility

External sources:

- GTD, "Managing projects with GTD":
  https://gettingthingsdone.com/2017/05/managing-projects-with-gtd/
- GTD, "The Elusive Inventory of Your Projects":
  https://gettingthingsdone.com/wp-content/uploads/2014/10/Project_Inventory.pdf
- Obsidian Help, "Properties":
  https://obsidian.md/help/properties
- Obsidian Help, "Create a base":
  https://obsidian.md/help/bases/create-base
- Obsidian Help, "Bases syntax":
  https://obsidian.md/help/bases/syntax
- Dataview, "Adding Metadata":
  https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/
- Dataview, "Metadata on Tasks and Lists":
  https://blacksmithgu.github.io/obsidian-dataview/annotation/metadata-tasks/
- Tasks User Guide:
  https://publish.obsidian.md/tasks/

## Recommended Solution

Adopt first-class GTD project notes as the canonical representation in Bob:
one note per committed multi-step outcome, selected by `type: project`, with
`status`, `area`, `priority`, `parent`, optional `project_key`, optional
`legacy_markers`, and a clear outcome statement.

Use the project note as the durable inventory/support record. Put rich thinking,
constraints, future dependent actions, reference links, and completion criteria
in the body. Keep current next actions as ordinary `#task` checkbox lines using
the vault's existing Dataview task format.

When a task is inside the project note, the file gives it project context. When
a task is captured elsewhere, add `[project:: [[project-note]]]`. Use
`project_key` only as a bridge to legacy `+marker` material, not as the main
human-facing project identity.

Build `projects.base` as the Projects List for Weekly Review, with active,
waiting, paused, someday, and done views. Keep Dataview/Tasks for action
dashboards. Add a stalled-project check after the schema has enough coverage.

Promote legacy material incrementally during Weekly Review: convert a `prj_*`
hub or generated marker into a canonical project note only when it represents a
current GTD commitment with a desired outcome. Leave the rest as reference and
search indexes.
