---
create_time: 2026-06-12
status: research
topic: Post-implementation review of Bob vault projects support + domain best practices (consolidated)
---
# Research: Projects Support Post-Implementation Review & Best Practices

Consolidated from two independent research runs (2026-06-12); all implementation inconsistencies below were verified
directly against the code, and the live-vault numbers come from read-only runs against `~/bob`.

## Question

The GTD "projects" system planned in `gtd_projects_bob_obsidian_consolidated.md` (2026-06-08) and the
`obsidian_projects` epic has now been built and extended. What does current (2025–2026) best practice in this domain
say, and what improvements should be made to the as-built system?

## As-Built System (reviewed 2026-06-12)

- **Project notes**: `type: "[[project]]"`, `parent` (area/project), `status: wip|waiting|done|canceled`,
  `priority` (int, absent = 0), machine-maintained `task_count`/`open_task_count`. Body: `^prj` completion-criteria
  task on the first body line, `## Tasks`, `## Project Support`. Documented in `~/bob/project.md`; `~/bob/area.md`
  holds the area type.
- **Dashboards**: `~/bob/projects.base` (4 views: Active & Waiting, Waiting, Closed, All; emoji status/priority
  badges, task-progress column from materialized counts) embedded in `~/bob/dash.md` alongside a Tasks-plugin query
  and `refs.base`.
- **Creation flows** (`bob-navigation-hotkeys` plugin): `Ctrl+Shift+N` creates a project from
  `_templates/new_project.md` with parent-type validation (active note must be type area/project);
  `Ctrl+Shift+Alt+N` promotes the selected task into a project (task text → `^prj` criteria, `[p::N]` carried over,
  block-ID-derived or auto-generated `<source>_<X>` filenames, backlinks rewritten to `[[new_project^prj]]`, source
  task deleted).
- **Count automation** (`bob-project-tasks` plugin): on debounced (250ms) `metadataCache.changed`, recounts the
  `## Tasks` section (`#task` checkbox lines; open statuses ` `, `/`, `B`; code fences skipped) and writes
  `task_count`/`open_task_count` via `processFrontMatter` only on change; cleans the fields off non-projects; manual
  "Recount all project tasks" command.
- **CLI** (`bob projects list|sync`, `src/native/projects.rs`): `list` reports per-project status, open/unprioritized
  counts, and `^prj` state (Missing/Open/Done/Canceled/Malformed/Multiple). `sync` (with `--dry-run`): checks
  `^prj` done/canceled → frontmatter `status`; removes deprecated `[scheduled::]`; manages dash visibility by
  adding `[p::2]` to `^prj` when the project has unprioritized tasks or open sub-projects (its work already
  surfaces) and removing it when fully triaged (so the `^prj` task itself surfaces on `dash.md` — a built-in
  stall-prevention mechanism); maintains a canonical, sorted, deduped `🧩 **Sub-projects:**` bullet under `^prj`
  from `parent` backlinks.

Read-only live checks against `~/bob` (2026-06-12): `bob projects list` found 7 active projects (0 waiting/done/
canceled) with no scan errors; `bob projects sync --dry-run` reported 0 status updates, 0 `^prj` edits, 0 warnings;
`cargo test projects` passed. The current vault already satisfies the `^prj` sync contract.

## Verified Implementation Inconsistencies

Confirmed directly in code on 2026-06-12:

1. **Placeholder mismatch**: the CLI only recognizes `<short_project_completion_criteria_goes_here>`
   (`projects.rs:19-20`, used at `projects.rs:1715` to warn about placeholder criteria), but
   `_templates/new_project.md:8` and the `bob-navigation-hotkeys` creation path emit
   `(REPLACE WITH PROJECT COMPLETION CRITERIA)`. Projects created via `Ctrl+Shift+N` with an unedited placeholder
   are never flagged and can keep the placeholder indefinitely. `project.md` documents the CLI's string.
2. **Open-status semantics diverge**: plugin treats only ` `, `/`, `B` as open (`bob-project-tasks/main.js:15`);
   the CLI treats anything not `x`/`X`/`-` as open (`projects.rs:419-427`). A task with any other custom status
   char counts as open to the CLI but closed to the plugin.
3. **Counting scope diverges**: the plugin counts only the `## Tasks` section (`taskSectionLines()`), so the `^prj`
   task (above the title) is excluded from `task_count`; CLI `list`'s open count scans the whole body
   (`projects.rs:1473-1495`) and includes `^prj`. "Open tasks" in `projects.base` and `bob projects list` are
   different numbers by construction. Observed live: `sase_install` shows CLI `OPEN=4` vs dashboard
   `open_task_count=3` (CLI includes `^prj`); `needs_attn_tasks` shows CLI `OPEN=15` vs `open_task_count=0`
   (all its task lines sit outside `## Tasks`, so the dashboard silently reports an empty project).
4. **Stale scheduled-field docs**: `docs/projects.md:68-83` correctly describes the current behavior (sync removes
   `[scheduled::]`; `[p::2]` drives dash surfacing), but `README.md:136-143` and `~/bob/project.md:27-29` still
   describe the retired behavior where projects with no open P0 tasks get `[scheduled::YYYY-mm-dd]` appended —
   a field `sync` now actively removes.
5. **Template/heading drift**: `_templates/new_project.md:13` creates `## Project Notes`; `project.md:31` and the
   epic's design contract specify `## Project Support`, and existing notes follow the template.
6. **Missing `[created::]`**: the template's `^prj` task has no `[created::]` stamp even though `project.md`
   requires it for hand-created tasks (Tasks plugin only auto-stamps tasks created through its own modal). The
   template's sample `## Tasks` entry does include it.
7. **Parent-type validation asymmetry**: the hotkey plugin enforces parent type ∈ {area, project}
   (`PROJECT_PARENT_TYPE_BASENAMES`, `main.js:24`); the CLI accepts any `parent: [[...]]` without validation, so
   drift introduced by hand-edits is never reported.
8. **Parent/sub-project identity is stem-based**: the CLI resolves `parent` links to a lowercase file stem and
   writes generated child links as `[[ChildStem]]`. Fine for today's root-level project notes, but it collides if
   projects move into folders or two notes share a basename — and nothing checks for duplicate stems.
9. **Tag-boundary regex duplication**: `#task` boundary detection is independently implemented in JS regex
   (`main.js:14`) and Rust functions (`projects.rs:1770-1796`). Currently aligned, but a fragile duplication.

## Research Findings

### A. GTD canon & mature Obsidian GTD systems

- **The stall invariant is three-way, not "has an open task"**: a project is current if it has a next action OR a
  waiting-for OR a calendar/scheduled item; only a project with none of the three is stalled
  ([Managing Projects with GTD](https://gettingthingsdone.com/2017/05/managing-projects-with-gtd/)). The
  best-regarded plugin encoding ([obsidian-gtd-no-next-step](https://github.com/saibotsivad/obsidian-gtd-no-next-step))
  uses red badge = no next-step and no waiting-for; gray badge = waiting-for only (blocked, not broken).
- **Every mature status taxonomy includes an on-hold/someday state** distinct from active/done: Mandalivia uses
  `Open|Inbox|Hold|Dropped|Done` ([mandalivia.com](https://www.mandalivia.com/obsidian/weekly-project-review-with-claude-code-and-obsidian-cli/)),
  Bases-era templates use `On Hold`/`Cancelled` ([wanderloots](https://wanderloots.com/obsidian-bases-project-management/),
  [Moy's template](https://medium.com/@moyfoxther/an-obsidian-bases-template-for-project-tracking-c4ac5351fbe0)),
  obsidian.rocks uses `#project/soon` ([obsidian.rocks](https://obsidian.rocks/how-to-manage-projects-in-obsidian/)).
  GTD reviews Someday/Maybe weekly in "Get Creative" but exempts it from the next-action invariant
  ([episode 54](https://gettingthingsdone.com/2019/10/david-allen-on-someday-maybe-and-incubation-lists/)).
  Bob's enum (`wip|waiting|done|canceled`) has no such state.
- **Staleness is a second, distinct failure mode**: a project can have open tasks yet be stalled because nothing
  moved. Mandalivia stores a per-project `review-cycle` (days int, default 14) and flags projects whose last
  activity is older; org-mode's org-review formalizes last-review + interval → computed next review
  ([org-review](https://mirrors.ocf.berkeley.edu/melpa/org-review-readme.txt)). The Weekly Review remains GTD's
  sanctioned stall detector — review every project and ensure a current next action exists
  ([checklist PDF](https://gettingthingsdone.com/wp-content/uploads/2014/10/Weekly_Review_Checklist.pdf)).
- **Waiting-for best practice**: record the **date delegated** on each waiting item to drive nudge decisions
  ([official tip](https://gettingthingsdone.com/2012/01/cool-gtd-tip-for-tracking-waiting-for-items-in-outlook/)).
- **Outcome statements**: phrase project completion as a binary-checkable outcome
  ([episode 38](https://gettingthingsdone.com/2018/02/episode-38-power-outcome-thinking/)) — Bob's `^prj` task is
  exactly this pattern; the placeholder-warning fix (above) is what enforces it. The single `^prj` completion task
  as the human interaction point also matches GTD's clarify step (next action ≠ project) and PARA's project/area
  split, which Bob's `project`/`area` types mirror ([PARA](https://fortelabs.com/blog/para/)).
- **Priority & daily dashboards**: numeric priority sorts natively and is the community norm for tooling; systems
  use project priority to gate *dashboard visibility*, not as a commitment hierarchy — the weekly review re-decides
  it. Tasks plugin ≥7.7.0 can filter by containing-note properties
  (`task.file.property('priority')`, [docs](https://publish.obsidian.md/tasks/Getting+Started/Obsidian+Properties)),
  an alternative to denormalizing priority onto task lines.
- **Sub-projects**: community consensus is bottom-up `parent` links in frontmatter (child edits never touch the
  parent) with rollups reconstructed by queries; nobody published automatic status write-back to parents — Bob's
  CLI-materialized sub-project line and priority rollup is ahead of published practice
  ([forum 34430](https://forum.obsidian.md/t/dataview-rollup-for-project-management/34430)).

### B. Obsidian Bases capabilities (Obsidian 1.13.1, June 2026)

- **Embedding a specific view is supported**: `![[File.base#View]]`
  ([create-base help](https://obsidian.md/help/bases/create-base)). `dash.md` currently embeds the bare base and
  relies on view order.
- **`this` context works in embedded bases**: an embedded base resolves `this` to the embedding note, so one
  reusable base with `file.hasLink(this.file)` (officially documented) or `note.parent == this.file.asLink()`
  (community-verified) gives per-area/per-parent project lists inside area notes
  ([Bases syntax](https://obsidian.md/help/bases/syntax)).
- **Cross-note lookups exist since 1.9.7**: `parent.asFile().properties.status` works in formulas, with documented
  caveats: performance cost and no auto-refresh when the looked-up note changes
  ([forum 101990](https://forum.obsidian.md/t/bases-formula-cross-note-lookup-rollup/101990/4)).
  `file.backlinks` works and is widely used but is **absent from the official function reference** — semi-supported.
- **Date math for staleness is first-class**: `file.mtime > now() - "1 week"` is the documented example shape;
  `date.relative()` renders "3 weeks ago" ([functions](https://obsidian.md/help/bases/functions)).
- **Still missing natively**: conditional formatting/colored cells (open FR — emoji badge columns remain the right
  pattern), text wrap in table cells, embed height/toolbar options (CSS-snippet workarounds only), select/enum
  property validation, and any access to note body content.
- **Task counts cannot be computed by Bases** — architectural, unchanged
  ([FR 101378](https://forum.obsidian.md/t/bases-display-access-and-or-filter-using-the-notes-content/101378)).
  Materializing counts into frontmatter is still the standard 2026 approach
  ([divby0.io](https://www.divby0.io/posts/obsidian-bases-task-management/)); Bob's architecture is validated.
- New view types since launch: List (1.10), Map (1.10, first-party Maps plugin); charts/kanban are community
  plugins. Summaries now include per-group summaries and custom formula summaries via the `values` keyword (1.10).

### C. Derived metadata & external-writer coexistence

- **`processFrontMatter` atomicity is in-app only** — its queue cannot see CLI writes, so plugin-after-CLI races
  are real ([API docs](https://docs.obsidian.md/Reference/TypeScript+API/FileManager/processFrontMatter)). It also
  normalizes YAML destructively (strips comments/quoting)
  ([forum 65851](https://forum.obsidian.md/t/yaml-properties-api-processfrontmatter-removes-alters-string-quotes-comments-types-formatting/65851)) —
  the CLI should emit Obsidian-normalized YAML so the two writers converge instead of ping-ponging.
- **Ownership partitioning by key/region** is the load-bearing safety property: Obsidian Sync's diff-match-patch
  merges disjoint regions cleanly but duplicates/garbles overlapping ones
  ([sync troubleshoot](https://obsidian.md/help/sync/troubleshoot)). Bob already partitions correctly (plugin:
  counts; CLI: status/`[p::2]`/sub-projects line) — keep it that way deliberately.
- **Write-only-when-changed** is what materializing tools converged on after sync-loop incidents (Linter's YAML
  timestamp cross-device loop, [issue 1414](https://github.com/platers/obsidian-linter/issues/1414); Dataview
  Serializer's compare-before-write). Both Bob writers already do this. Machine-owned key marking has no ecosystem
  standard; the closest prior art is a key prefix (`dv_` in
  [Dataview Properties](https://github.com/Mara-Li/obsidian-dataview-properties)).
- **CLI write-path hardening canon**: write to a **dot-prefixed** temp file in the same directory (Obsidian doesn't
  index dotfiles), fsync, re-stat the target and abort if changed since read (compare-and-swap), then `rename(2)`
  into place; `flock -n` around cron runs to serialize against self; **one coalesced write per file** per sync
  (each write is a separate watcher event/Sync revision; rapid successive writes are Sync's documented weak spot
  ([synch.run](https://synch.run/blog/obsidian-sync-conflicts/))).
- **Generated-region idempotency**: paired BEGIN/END markers (doctoc, Dataview Serializer) with
  regenerate-whole-region and dedupe-on-N-occurrences is the dominant convention; cog adds a checksum to detect
  human edits inside the region and refuses to clobber ([cog](https://cog.readthedocs.io/en/stable/running.html)).
  Bob's single visible marker line is the weakest variant but fits its one-bullet payload; the existing
  normalize/dedupe pass covers the Sync-duplication failure mode — keep the rewrite anchored and deduplicating.
- **Tasks-plugin line grammar**: Tasks parses task lines backwards from the end and stops at the first unrecognized
  token, so the block ID must stay **last** and `[p::2]` insertion/removal must not land after Tasks-recognized
  metadata ([known limitations](https://publish.obsidian.md/tasks/Support+and+Help/Known+Limitations)). External
  edits to `[key:: value]` fields are within ecosystem norms. Avoid creating two byte-identical task lines in one
  file — it breaks Tasks' checkbox-toggle targeting
  ([issue 3267](https://github.com/obsidian-tasks-group/obsidian-tasks/issues/3267)). Note also that Tasks
  auto-suggest only activates on lines carrying the global filter (`#task`)
  ([auto-suggest docs](https://publish.obsidian.md/tasks/Editing/Auto-Suggest)).
- **Unsafe window**: external writes to a note the user is actively editing can trigger bad merges
  ([forum 26090](https://forum.obsidian.md/t/bug-modified-externally-message-constantly-appears-erasing-my-text/26090));
  schedule cron syncs away from active editing hours where feasible, and consider the per-device "Create conflict
  file" Sync setting (1.9.7+) on the cron machine so bad merges are visible.

## Recommendations

Ordered by value/effort:

### P0 — consistency fixes (small, mechanical)

1. **Fix the placeholder mismatch**: make the template's friendlier
   `(REPLACE WITH PROJECT COMPLETION CRITERIA)` the canonical placeholder; have the CLI recognize both strings
   (cheap, covers legacy notes); update `project.md` (which documents the CLI string) and the Rust fixture tests.
2. **Update stale scheduled-field docs**: rewrite `README.md:136-143` and `~/bob/project.md:27-29` to describe the
   current `[p::2]` surfacing behavior and the generated `🧩 **Sub-projects:**` line (matching `docs/projects.md`).
3. **Rename `## Project Notes` → `## Project Support`** in `_templates/new_project.md` (matches `project.md`, the
   design contract, and GTD's "project support" term).
4. **Add `[created::<% tp.file.creation_date("YYYY-MM-DD") %>]`** to the template's `^prj` task.
5. **Unify task-count semantics**: pick one rule for open status — recommend the CLI's "not `x`/`X`/`-` ⇒ open"
   (robust to new custom statuses) — and apply it in the plugin. For scope, make workload counts canonical as
   "`## Tasks` only, excluding `^prj`" (the plugin's model); the CLI should either read the materialized
   frontmatter counts or share the same section parser, and any whole-body scan it keeps for surfacing logic
   should be labeled distinctly (e.g. `UNPRI`/dash blockers), not presented as the project's task count. Warn on
   body `#task` lines outside `## Tasks` so a project like `needs_attn_tasks` can't look empty in `projects.base`
   while the CLI sees 15 open tasks.
6. **CLI parent validation**: `bob projects list`/`sync` should warn when a project's `parent` is missing or
   resolves to a non-area/non-project note (parity with the hotkey's `PROJECT_PARENT_TYPE_BASENAMES` check), and
   warn on duplicate project stems or non-root project files while parent matching stays stem-based (or move to
   path-aware matching). A consolidated `bob projects doctor` (or `sync` warnings) covering: placeholder `^prj`,
   body tasks outside `## Tasks`, missing `status`, missing/invalid `parent`, duplicate stems, stale `scheduled`
   on open `^prj` — would catch all of the contract drift found in this review mechanically.

### P1 — GTD gaps (highest behavioral value)

7. **Add a `someday` (or `hold`) status**: exempt from `^prj`-surfacing/stall logic, excluded from Active views,
   given its own `projects.base` view reviewed weekly ("Get Creative"). Every mature taxonomy has this state; today
   a paused project must masquerade as `waiting` (which means "blocked on someone") or stay `wip` (polluting the
   dashboard).
8. **Three-way stall reporting in `bob projects list`/`sync`**: red = `wip` with no open task, no waiting item, no
   scheduled item (today partially covered by `^prj` surfacing); gray = only waiting items; stale = no file activity
   within a per-project `review_cycle` (frontmatter int days, default ~14). Add a `🥶 Stale` column or view to
   `projects.base` via `file.mtime < now() - "2w"` and `date.relative()` — native, no new automation. If richer
   review metadata is wanted later (`last_reviewed`, `closed` synced from the checked/canceled `^prj`), keep `^prj`
   as the source of truth and treat the frontmatter as derived.
9. **Waiting-for dates**: adopt `[waiting:: YYYY-MM-DD]` (date delegated) on waiting tasks and/or a `waiting_since`
   property set when `status: waiting`, so the Waiting view can sort by age and surface nudges.

### P2 — exploit current Bases features (cheap polish)

10. **Pin the dash embed to a named view**: `![[projects.base#🚀 Active & Waiting]]` instead of relying on view
    order.
11. **Per-area project lists**: one reusable embedded base (or `base` code block) in area notes filtered by
    `note.parent == this.file.asLink()` — gives each area its own live project index for reviews.
12. **Optional sub-project rollup column** via `parent.asFile().properties.status` lookups (mind the
    no-auto-refresh caveat) — could eventually replace parts of the CLI's materialized sub-project line, but the
    materialized line also serves non-Obsidian consumers, so keep both for now.

### P3 — robustness hardening (CLI write path)

13. **Atomic CAS writes in `bob projects sync`**: dot-prefixed same-dir temp file + fsync + re-stat (abort if
    changed since read) + rename; coalesce all per-file edits (status, `[p::2]`, marker line) into one write — and
    verify this is already the case; `flock -n` if/when run from cron.
14. **Preserve Tasks line grammar invariants** when editing `^prj` lines: block ID last; never duplicate an
    identical task line; keep the existing dedupe/normalize pass for the `🧩 **Sub-projects:**` marker.
15. **Share the task-grammar definition**: extract the `#task` boundary + status rules into one spec (a small
    fixture-based conformance test run against both the JS plugin and Rust CLI would prevent silent drift).
16. **Sync settings on the cron machine**: prefer "Create conflict file" over auto-merge; schedule syncs outside
    active editing hours.

## Explicit Non-Recommendations

- **Do not move task counting into Bases/Dataview** — Bases still cannot read body content (2026), and the counts
  also feed `bob` CLI consumers. The materialized-frontmatter architecture is validated by current practice.
- **Do not adopt a project-management plugin or task-as-note model (TaskNotes)** — would fork the task data model;
  reaffirmed from the 2026-06-08 research.
- **Do not switch the marker line to BEGIN/END comment markers yet** — the payload is a single indented bullet under
  `^prj`; paired markers would break the visual nesting. Revisit only if the generated region grows.
- **Do not add static priority ranks beyond the current scheme** — GTD canon resists static project priority;
  current use (visibility gating, weekly re-decision) matches community best practice.

## Suggested Acceptance Tests

- A project created from the template triggers the placeholder warning until the completion criterion is replaced.
- A project with `#task` lines outside `## Tasks` reports a warning, or has matching CLI/dashboard counts by design.
- `README.md`, `docs/projects.md`, and `~/bob/project.md` all describe the same `[p::2]` surfacing behavior.
- A duplicate project stem fixture either errors or produces path-aware generated sub-project links.
- The same fixture task lines produce identical open/closed/total counts from the JS plugin and the Rust CLI.
- `bob projects sync --dry-run --bob-dir ~/bob` remains clean after the docs/template cleanup.

## Sources

Prior local research: `sdd/research/202606/gtd_projects_bob_obsidian_consolidated.md` (2026-06-08).
Code reviewed: `~/bob/projects.base`, `~/bob/dash.md`, `~/bob/project.md`, `~/bob/area.md`,
`~/bob/_templates/new_project.md`, `~/bob/.obsidian/plugins/bob-project-tasks/main.js`,
`~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`, `src/native/projects.rs`, `docs/projects.md`, `README.md`.
Live read-only verification: `bob projects list --bob-dir ~/bob`, `bob projects sync --dry-run --bob-dir ~/bob`,
`cargo test projects`.

External (key; per-claim links inline above):

- GTD: [Managing Projects with GTD](https://gettingthingsdone.com/2017/05/managing-projects-with-gtd/) ·
  [Linking Next Actions and Projects](https://gettingthingsdone.com/2020/06/the-gtd-approach-to-linking-next-actions-and-projects/) ·
  [Weekly Review Checklist](https://gettingthingsdone.com/wp-content/uploads/2014/10/Weekly_Review_Checklist.pdf) ·
  [Outcome Thinking](https://gettingthingsdone.com/2018/02/episode-38-power-outcome-thinking/) ·
  [Someday/Maybe & incubation](https://gettingthingsdone.com/2019/10/david-allen-on-someday-maybe-and-incubation-lists/) ·
  [PARA](https://fortelabs.com/blog/para/)
- Obsidian GTD systems: [mandalivia.com weekly project review](https://www.mandalivia.com/obsidian/weekly-project-review-with-claude-code-and-obsidian-cli/) ·
  [alangrainger/obsidian-gtd](https://github.com/alangrainger/obsidian-gtd) ·
  [obsidian-gtd-no-next-step](https://github.com/saibotsivad/obsidian-gtd-no-next-step) ·
  [obsidian.rocks projects](https://obsidian.rocks/how-to-manage-projects-in-obsidian/) ·
  [wanderloots Bases PM](https://wanderloots.com/obsidian-bases-project-management/)
- Bases (Obsidian 1.13.1): [syntax](https://obsidian.md/help/bases/syntax) ·
  [functions](https://obsidian.md/help/bases/functions) ·
  [create-base / embeds](https://obsidian.md/help/bases/create-base) ·
  [cross-note lookup thread](https://forum.obsidian.md/t/bases-formula-cross-note-lookup-rollup/101990/4) ·
  [body-content FR](https://forum.obsidian.md/t/bases-display-access-and-or-filter-using-the-notes-content/101378)
- Properties/Dataview/Tasks: [Obsidian Properties](https://obsidian.md/help/properties) ·
  [Dataview inline fields](https://blacksmithgu.github.io/obsidian-dataview/annotation/add-metadata/) ·
  [Tasks introduction](https://publish.obsidian.md/tasks/Introduction) ·
  [Tasks auto-suggest](https://publish.obsidian.md/tasks/Editing/Auto-Suggest)
- Coexistence/sync: [processFrontMatter API](https://docs.obsidian.md/Reference/TypeScript+API/FileManager/processFrontMatter) ·
  [YAML normalization](https://forum.obsidian.md/t/yaml-properties-api-processfrontmatter-removes-alters-string-quotes-comments-types-formatting/65851) ·
  [Sync conflict behavior](https://obsidian.md/help/sync/troubleshoot) ·
  [Linter sync loop](https://github.com/platers/obsidian-linter/issues/1414) ·
  [cog generated-region checksums](https://cog.readthedocs.io/en/stable/running.html) ·
  [Tasks known limitations](https://publish.obsidian.md/tasks/Support+and+Help/Known+Limitations) ·
  [Tasks toggle-targeting issue](https://github.com/obsidian-tasks-group/obsidian-tasks/issues/3267)
