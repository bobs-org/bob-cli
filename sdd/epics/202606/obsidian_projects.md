---
create_time: 2026-06-11 16:58:08
status: wip
prompt: sdd/prompts/202606/obsidian_projects.md
---
# Plan: Add "Projects" Support to the Bob Obsidian Vault

## Goal

Introduce a first-class GTD "project" concept to the Bob vault. A project is any note with `type: "[[project]]"`
frontmatter. Deliverables:

1. A beautiful `~/bob/projects.base` dashboard of active/waiting projects, including per-project task counts.
2. A new `~/bob/dash.md` dashboard note that takes over the "Tasks", "Reading List", and (new) "Projects" sections from
   `~/bob/_templates/daily.md` (and from the current day's daily note).
3. A new `~/bob/area.md` type note (GTD areas of responsibility) and a migration of every note that holds open Obsidian
   tasks to `type: "[[area]]"` or `type: "[[project]]"`.
4. A `~/bob/_templates/new_project.md` template plus a `<Ctrl+Shift+N>` keymap that creates a project note the same way
   the built-in `Cmd+N` creates a regular note, with parent-type validation and a toast error on misuse.

The work is split into **4 phases**, each completed by a distinct agent instance. Phases are ordered by dependency; each
phase ends with a focused `/sase_git_commit` of only its own files.

## Context Reviewed

- `memory/short/sase.md` (loaded via AGENTS.md). `memory/long/cli_rules.md` is **not** required: no `bob` CLI
  subcommands or options are added by this plan (all automation lives in the vault's Obsidian plugins). If any phase
  agent deviates and touches the CLI surface, it MUST first run `/sase_memory_read` on `memory/long/cli_rules.md`.
- `/home/bryan/bob/AGENTS.md`: the vault is synced by Obsidian Sync and is currently _very_ dirty (hundreds of unrelated
  modified files, e.g. 2023 dailies and `.obsidian/community-plugins.json`). Agents must inspect `git -C ~/bob status`
  before editing, never revert/stage unrelated changes, and commit only task files via `/sase_git_commit` before
  terminating.
- Bryan's own design notes for this effort: `~/bob/bob_projects.md` (incl. open task `^b13861` "Convert the
  [[bob_projects]] note into the canonical project file!") and
  `~/bob/ref/chat/gtd_projects_bob_obsidian_consolidated.md`.
- Vault inventory: templates (`_templates/daily.md`, `_templates/new_note.md`), type notes (`type.md`, `project.md`,
  `day.md`; `area.md` missing), existing `.base` house style (`refs.base`, `eat.base`), `.obsidian/hotkeys.json`,
  Templater settings (root folder template → `_templates/new_note.md`, `trigger_on_file_creation: true`),
  `bob-navigation-hotkeys` plugin (~4.1k lines; already creates notes from templates via Templater's
  `create_new_note_from_template`), metadata-menu preset field `type` (suggests children of `type.md` — `area.md` will
  be picked up automatically once it has `parent: "[[type]]"`), Tasks plugin (`globalFilter: "#task"`, custom statuses
  `/`=IN_PROGRESS, `B`=Blocked, `-`=Cancelled).
- Current `type` taxonomy in the vault: `[[day]]` (1069), `[[ref]]` (306), `[[restaurant]]` (91), `[[done]]` (7),
  `[[project]]` (2: `job.md`, `obsidian.md`), `[[inbox]]` (2).
- Full inventory of non-daily, non-archive, non-generated notes containing open `- [ ]` tasks (enumerated in Phase 1
  below).
- Prior related plans: `sase_plan_obsidian_cmd_n_new_note_template.md` (how Cmd+N + Templater folder template works
  today), `sase_plan_refs_base_view.md`, `sase_plan_eat_base.md`.
- Obsidian Bases capability check (official docs, June 2026): Bases formulas can only read file metadata and frontmatter
  — there is **no** `file.tasks` accessor. Task counts therefore must be materialized into frontmatter by automation
  (Phase 3) for `projects.base` to display them.

## Design Contract (shared by all phases)

All phase agents MUST honor this contract so independently-implemented pieces compose:

- **Project note frontmatter**:
  - `type: "[[project]]"` (required)
  - `parent: "[[<area-or-project>]]"` (required; must point at a note of type `[[area]]` or `[[project]]`)
  - `status: wip | waiting | done | canceled` (required; new projects default to `wip`)
  - `priority: <integer ≥ 0>` (optional; **absent means 0**; 0 is the highest/default band, matching Bryan's
    P0/P1/`[p::N]` conventions; sort ascending)
  - `task_count: <N>` / `open_task_count: <N>` (machine-maintained by Phase 3 automation; humans never edit)
- **Area note frontmatter**: `type: "[[area]]"` + `parent` (a broader area, topic note, or `[[h2_role]]`). Areas do not
  carry `status`/`priority`/task counts.
- **Project body structure**: open tasks live under a `## Tasks` H2; supporting material lives under
  `## Project Support`. Task counts are computed **only** from the `## Tasks` section.
- **What counts as a task**: a checkbox list item carrying the Tasks-plugin global filter `#task` (at any indent level)
  inside the `## Tasks` section. `task_count` = all such tasks; `open_task_count` = those with status symbol ` ` (TODO),
  `/` (IN_PROGRESS), or `B` (Blocked) — i.e. not done `x` and not canceled `-`.
- **Vault hygiene**: check `git -C ~/bob status --short` before editing; never reformat or normalize unrelated
  frontmatter; preserve `generated_from_zorg` / `zorg_*` metadata and existing block IDs (`^anchors` are referenced from
  daily Pomodoro logs — never move or rewrite task lines unnecessarily); commit only phase files with
  `/sase_git_commit`.
- **Verification limits**: agents cannot run the Obsidian GUI. Each phase performs static verification (JSON via `jq`,
  JS via `node --check`, YAML parse via `python3 -c "import yaml,sys; yaml.safe_load(...)"`, targeted `grep` audits) and
  ends with a short "manual acceptance" checklist for Bryan.

---

## Phase 1 — Area/Project Type Foundation & Note Migration

**Purpose**: establish the `area`/`project` type system and migrate every note holding open tasks onto it.

### Scope (files)

- **Create `~/bob/area.md`** — the GTD area-of-responsibility type note. Mirror `project.md`'s shape:
  `parent: "[[type]]"`, `created`, and a short body: areas are ongoing spheres of responsibility with no end state (e.g.
  `[[job]]`); area notes link here as their `type`; projects/sub-areas point at an area via `parent`.
- **Update `~/bob/project.md`** — document the new project contract (frontmatter schema from the Design Contract,
  `## Tasks` / `## Project Support` structure, creation via `<Ctrl+Shift+N>`, and that `projects.base` is the
  dashboard). Keep `parent: "[[type]]"`.
- **Explicit re-typings**:
  - `~/bob/job.md`: `type: "[[project]]"` → `type: "[[area]]"`. Drop project-only props (`status: active`,
    `priority: P1`, legacy `area: job`, `project` tag); give it a sensible `parent` (recommend `[[h2_role]]`, the GTD
    Horizon-2 note).
  - `~/bob/obsidian.md`: no longer a project — remove `type`, `status: active`, legacy `area: dev`, and the `project`
    tag entirely (it stays a plain topic note; it has no open tasks).
  - `~/bob/bob.md`: becomes a project in obsidian.md's place — add `type: "[[project]]"`, `status: wip` (parent
    `[[obsidian]]` already correct). Its tasks already live under `## Tasks`.
  - `~/bob/bob_projects.md`: convert into a canonical project (`type: "[[project]]"`, `status: wip`, fix
    `parent: "[[bob.md|bob]]"` → `parent: "[[bob]]"`), satisfying Bryan's own task `bob_projects#^b13861` (the phase may
    check that task off with a `[completion:: <date>]` stamp).
- **Convert the remaining open-task notes.** Exclusions (justified "best judgement" carve-outs from "ALL"):
  daily/monthly/yearly notes (type `[[day]]`), `done/` archives, `_generated/` query output, `_templates/`, and
  `ref/`-typed reading notes (their open tasks are reading tasks already surfaced by `refs.base`; re-typing them would
  break the reading-list pipeline). Everything else converts **in place** (tasks are never relocated — block anchors are
  referenced elsewhere). Proposed classification table (the agent verifies each file's content and may deviate with
  documented rationale in its commit message):

  | Note                                                                                                                                                                                                    | Proposed type             | Rationale                                                                                                              |
  | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
  | `sase.md`                                                                                                                                                                                               | area                      | Bryan's main ongoing endeavor; sub-projects hang off it                                                                |
  | `recur.md`                                                                                                                                                                                              | area                      | recurring chores, never "done"                                                                                         |
  | `gtd_daily.md`                                                                                                                                                                                          | area                      | daily habit recurrences                                                                                                |
  | `mac_inbox.md`                                                                                                                                                                                          | area                      | ongoing capture stream                                                                                                 |
  | `needs_attn_tasks.md`                                                                                                                                                                                   | area                      | rolling triage backlog from zorg                                                                                       |
  | `now_gtd.md`, `now_dev.md`, `now_sase.md`                                                                                                                                                               | area                      | per-topic "next actions" lists, ongoing                                                                                |
  | `soon_zorg.md`, `soon_dev.md`, `prj/rap/soon_rap.md`                                                                                                                                                    | area                      | per-topic someday/soon backlogs                                                                                        |
  | `*_ref.md` with open tasks (`ai_ref`, `dev_ref`, `nvim_ref`, `zorg_ref`, `work_ref`, `obsidian_ref`, `org_mode_ref`, `mcp_ref`, `gemini_cli_ref`, `keyboard_maestro_ref`, `alfred_ref`, `asciidoc_ref`) | area                      | reference hubs under ongoing maintenance                                                                               |
  | `sase_blog.md`                                                                                                                                                                                          | project (`wip`)           | finite blog-series effort with tasks                                                                                   |
  | `sase_install.md`, `sase_version.md`                                                                                                                                                                    | project                   | completable SASE efforts (agent confirms status from content)                                                          |
  | `prj_yserve.md`, `prj_zorg.md`                                                                                                                                                                          | project                   | legacy zorg project notes (agent picks `waiting`/`canceled`/`wip` from content; ex-job projects are likely `canceled`) |
  | `gkeep_gdocs_inbox_dump.md`                                                                                                                                                                             | project (`wip`)           | one-time triage dump, done when emptied                                                                                |
  | book notes: `clean_arch.md`, `balance_coupling.md`, `soft_arch_hard_parts.md`, `cat_theory_for_devs.md`, `how_to_read_a_book.md`, `outlive.md`, `think_fast_and_slow.md`                                | project (`wip`/`waiting`) | finite reading efforts                                                                                                 |

  For every converted **project**, ensure `parent` exists and points at an area/project (add the obvious topic parent if
  missing) and `status` is set; if its open tasks are not already under a `## Tasks` H2, add the heading above the
  existing task block _without_ moving task lines (renaming an existing heading is allowed only after grepping for
  inbound `[[note#Heading]]` links).

- **Optional polish**: register `priority` as a `number` property in `~/bob/.obsidian/types.json` (file is currently
  clean; additive edit only).

### Verification

- Re-run the open-task audit: every `- [ ]`-containing note outside the exclusion list has `type` `[[area]]` or
  `[[project]]`.
- All project notes YAML-parse and carry `type`/`parent`/`status`.
- `git -C ~/bob diff` touches only the files above; commit via `/sase_git_commit`.
- Manual acceptance: spot-check `job.md`, `bob.md`, `obsidian.md`, `bob_projects.md` render correctly in Obsidian;
  metadata-menu `type` suggestions now include `area`.

---

## Phase 2 — `new_project.md` Template & `<Ctrl+Shift+N>` Keymap

**Purpose**: a one-keystroke project-creation flow mirroring `Cmd+N`.

### Scope (files)

- **Create `~/bob/_templates/new_project.md`** (Templater template), modeled on `new_note.md`:
  - Frontmatter: `parent` (link to the _creating_ note — see command behavior below), `type: "[[project]]"`,
    `status: wip`, `created` (same Templater expression as `new_note.md`). `priority` is intentionally omitted (absent =
    0 per the Design Contract).
  - Body: `# <% tp.file.title %>`, then `## Tasks` and `## Project Support` sections.
- **Add a `create-project-note` command to `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`** (this plugin
  already owns template-based note creation and uses Templater's `create_new_note_from_template` API):
  1. Resolve the "creating" note = the active markdown file. Read its `type` from `app.metadataCache`.
  2. If the active note's type is not `[[area]]` or `[[project]]` (or there is no active note), show a clear
     `new Notice(...)` toast error and abort — no file is created.
  3. Otherwise create a new untitled note in the vault root from `_templates/new_project.md` (same UX as `Cmd+N`:
     untitled file, cursor ready for rename), and ensure `parent` is set to a link to the creating note (either by the
     template's Templater expression reading the active file, or by the command via `fileManager.processFrontMatter`
     after creation — agent's choice; the command must guarantee correctness even though Templater's root
     folder-template rule auto-applies `new_note.md` to _empty_ new files, so creation must go through the
     Templater-from-template path, not an empty-file path).
- **Bind it in `~/bob/.obsidian/hotkeys.json`**: `Ctrl+Shift+N` → `bob-navigation-hotkeys:create-project-note` (additive
  JSON edit; file is dirty — preserve existing entries).

### Verification

- `node --check` on `main.js`; `jq .` on `hotkeys.json`; template YAML parses after stripping Templater tags.
- Manual acceptance: from a project/area note, `Ctrl+Shift+N` creates an untitled project note with correct
  `parent`/`type`/`status` and both sections; from a non-area/project note (e.g. a daily note), it shows the error toast
  and creates nothing; `Cmd+N` behavior is unchanged.
- Commit via `/sase_git_commit`.

---

## Phase 3 — Task-Count Automation (`bob-project-tasks` plugin) & Backfill

**Purpose**: materialize per-project task counts into frontmatter so Bases can display them (Bases has no task access of
its own).

### Scope (files)

- **Create a new minimal vault plugin `~/bob/.obsidian/plugins/bob-project-tasks/`** (`manifest.json`, `main.js`; follow
  the conventions of the existing hand-maintained plugins):
  - On `metadataCache.changed` (debounced) for notes whose `type` is `[[project]]`: parse the `## Tasks` section,
    compute `task_count` / `open_task_count` per the Design Contract, and write them via
    `fileManager.processFrontMatter` **only when a value actually changed** (guards against infinite change-event/Sync
    churn loops).
  - When a note stops being a project, remove the two machine properties.
  - Register a manual command `Recount all project tasks` that sweeps every project note (recovery/backfill from inside
    Obsidian).
- **Enable it** in `~/bob/.obsidian/community-plugins.json` (file is dirty — append the entry, preserve the rest).
- **Backfill now, headlessly**: since the agent cannot run Obsidian, it computes counts for every current project note
  with a one-off script (same counting rules) and writes `task_count`/`open_task_count` into their frontmatter, so Phase
  4's base is populated immediately. The plugin keeps the values fresh thereafter.

### Verification

- `node --check main.js`; `jq` on `manifest.json`/`community-plugins.json`.
- Independently recompute counts for 3–5 sample projects (e.g. `bob.md`) by hand and compare to the backfilled
  frontmatter.
- Manual acceptance: enable the plugin, edit a task in a project's `## Tasks` section, watch the counts update; run
  `Recount all project tasks` once.
- Commit via `/sase_git_commit`.

---

## Phase 4 — `projects.base`, `dash.md`, and Daily-Note Migration

**Purpose**: the user-facing dashboard, and the daily-template slimdown.

### Scope (files)

- **Create `~/bob/projects.base`** following the vault's `.base` house style (emoji badges, formula columns, groupBy,
  summaries — see `refs.base`/`eat.base`). Design (Phase agent leads final polish; "make it beautiful"):
  - Global filter: `note.type == link("project")` and not in `_templates/`.
  - Formulas: `title_link` (note title as link), `status_badge` (🛠️ WIP / ⏳ Waiting / ✅ Done / ⚫ Canceled),
    `priority_badge` (`"P" + (priority or 0)`, e.g. 🔥 P0 / 🔼 P1 / 🔽 P2+), `tasks` progress (from
    `open_task_count`/`task_count`, e.g. `"☑️ 5 open / 12"`, with a done/empty fallback), `parent` displayed as
    "Parent".
  - Views (the **first** view is what `dash.md` transcludes, so it must be the active/waiting table):
    1. `🚀 Active & Waiting` — `status.containsAny("wip", "waiting")`, grouped by `parent`, sorted by priority
       (ascending) then `file.mtime` desc; columns: Project, Status, Priority, Tasks, Updated; summaries: Count.
    2. `⏳ Waiting` — waiting only.
    3. `✅ Closed` — done/canceled.
    4. `📦 All Projects` — grouped by status.
- **Create `~/bob/dash.md`**: frontmatter (`parent: "[[gtd]]"`, `created`, alias `Dashboard`), `# Dash` heading, then
  the three migrated sections:
  - `## Tasks` — the `tasks` codeblock from `_templates/daily.md`, with the date filter adapted for a non-daily file:
    replace `moment(query.file.filenameWithoutExtension, "YYYYMMDD")` with `moment()` (today) in the scheduled-date
    filter, and keep the other filters/grouping intact.
  - `## Reading List ([[refs.base]])` — `![[refs.base]]`.
  - `## Projects ([[projects.base]])` — `![[projects.base]]` (new section).
- **Update `~/bob/_templates/daily.md`**: remove the `## Tasks` and `## Reading List` sections (Pomodoros stays); add a
  `[[dash]]` link to the `prev | day | next` nav line so dailies are one hop from the dashboard.
- **Update the current day's daily note** (`~/bob/2026/<YYYYMMDD>.md` for the date the phase runs — it is `20260611.md`
  today but the agent must use the actual current date) with the same section removal + nav link, preserving everything
  else in the file (especially the Pomodoros log).

### Verification

- `projects.base` and `dash.md` frontmatter YAML-parse; base filters/formulas reference only properties defined in the
  Design Contract.
- Confirm `_templates/daily.md` still renders valid Templater frontmatter and that only the intended sections were
  removed (diff review).
- Manual acceptance: open `dash.md` — tasks list, reading list, and the active/waiting projects table all render; task
  counts and status badges look right; create a scratch project via `Ctrl+Shift+N` and watch it appear in the table;
  tomorrow's daily note no longer contains Tasks/Reading List but links to `[[dash]]`.
- Commit via `/sase_git_commit`.

---

## Risks

- **Vault is live and dirty** (Obsidian Sync). Every phase re-checks `git status` immediately before editing and touches
  only its own files; conflicts with concurrent Sync edits are possible — re-read files right before writing.
- **Frontmatter churn from auto-counts**: `processFrontMatter` writes trigger change events and Sync traffic. The
  changed-value guard in Phase 3 is mandatory; the debounce keeps rapid edits cheap.
- **`status` property is shared** with the reading-list pipeline (different value set, e.g. `unread`/`read`).
  `refs.base` filters by path and `projects.base` filters by `type`, so the sets cannot collide in views; but
  metadata-menu value suggestions may mix — acceptable, noted for Bryan.
- **Re-typing `job.md`/`obsidian.md`** removes zorg-era props (`area`, `P1`). Their `zorg_*` provenance metadata is
  preserved; only project-contract fields change.
- **Heading edits near tasks** can break `[[note#Heading]]`/`^block` references — phases never move task lines and grep
  for inbound links before renaming any heading.
- **Bases/Tasks behavior can only be smoke-tested by Bryan** — each phase's manual acceptance list keeps that explicit
  and small.

## Out of Scope (noted from Bryan's design notes, not requested here)

- A `bob projects` CLI command (status sync to a `^prj` task, crontab validation) — future work; would require the
  `memory/long/cli_rules.md` procedure.
- Converting `ref/`-typed reading notes or relocating their tasks.
- Workspace/tab configuration and any `tasks.base` file.
