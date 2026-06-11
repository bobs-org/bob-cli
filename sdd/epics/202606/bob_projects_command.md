---
create_time: 2026-06-11 18:16:00
status: done
bead_id: bob-cli-7
tier: epic
prompt: sdd/prompts/202606/bob_projects_command.md
---
# Plan: `bob projects` — Project ^prj Task Sync Command & Vault Migration

## Goal

Add a native `bob projects` subcommand that makes the `^prj` task the single interaction point for a project's
lifecycle, mirroring how the generated `^task` line drives `status` in `[[ref]]` notes via `bob highlights`:

1. Every project note (frontmatter `type: "[[project]]"`) carries a completion-criteria task anchored `^prj`:
   `- [ ] #task <short_project_completion_criteria> [p::2] ^prj`. The `_templates/new_project.md` template gains this
   line so all future projects start with it.
2. `bob projects sync` reconciles each project: a checked `^prj` (`[x]`/`[X]`) sets frontmatter `status: done`; a
   canceled `^prj` (`[-]`) sets `status: canceled`.
3. `bob projects sync` also surfaces stalled projects: when an active project has **no open P0 task** (every open
   `#task` carries an explicit `[p::N]` field — a task _without_ `p` is implicitly P0), the `^prj` task gets
   `[scheduled::YYYY-mm-dd]` (today) appended so it shows up in the daily task dashboard.
4. The vault is migrated: obviously legacy projects (no open Obsidian tasks) lose their `type` property; every remaining
   live project gets a hand-crafted `^prj` task.
5. Output is beautiful: colored, aligned, concise, with a read-only `bob projects list` overview.

The work is split into **3 phases**, each completed by a distinct agent instance. Phases 1–2 change only the `bob-cli`
repo; Phase 3 changes only the `~/bob` vault repo. Each phase ends with a focused `/sase_git_commit` of its own files,
and Phase 3 ends by closing the epic bead.

## Context Reviewed

- `memory/long/cli_rules.md` (via `sase memory read`, reason logged): excellent `-h/--help`, subcommands/options sorted
  alphabetically, **every public long option needs a short alias**, prefer beautiful colored output. Phase 1 and 2
  agents MUST re-run this `/sase_memory_read` procedure before touching the CLI surface.
- `src/runner.rs`: `SUBCOMMANDS` table (sorted; guarded by `subcommands_are_sorted_alphabetically`), `cli_styles()` ANSI
  palette, `AFTER_HELP` examples, delegate-subcommand pattern (`trailing_var_arg` → native module owns its own clap
  parser).
- `src/native/highlights_ref/mod.rs`: the `^task` precedent — `parse_pdf_task_line` (single `^task` per note is
  enforced; multiple → error), `parse_markdown_task_checkbox` (marks ` `/`x`/`X`/`-`), `PdfTaskStatus::target_status`
  (`x`→`read`, `-`→`abandoned`), and the malformed-line error style. `bob projects` mirrors the _semantics_ but stays
  self-contained (no refactor of the 7.5k-line highlights module; it also must accept the Tasks-plugin custom marks `/`
  and `B` as "open", which highlights deliberately rejects).
- `src/native/env.rs`: `bob_dir()` (`BOB_DIR` override, default `~/bob`), `current_datetime()` (`BOB_NOW`/`DATE`
  overrides — the testable "today" source).
- `src/native/collect_done.rs` (`move-done-tasks`): archives done/canceled tasks (block IDs included, with archive-side
  dedup/renames) from any note once it accrues ≥10 done tasks, skipping `done/`, `.git`, `.obsidian`. Consequence: a
  checked `^prj` line eventually migrates to `done/<note>_done.md`, so **terminal projects must never be required to
  still contain a `^prj` task**.
- Vault state (live inventory, re-verify at execution time):
  - Tasks plugin: `globalFilter: "#task"`, `taskFormat: dataview` (`[p::2]`, `[scheduled:: ...]` inline fields), custom
    statuses `/` (in progress), `B` (blocked), `-` (cancelled).
  - `bob-project-tasks` plugin tag regex `/(^|[\s([{])#task(?=$|[\s\])}:.,;!?])/` — the authoritative "is a task" test.
  - 15 notes with frontmatter `type: "[[project]]"` (all at vault root; full classification in Phase 3).
  - `bob_projects.md` already carries Bryan's prototype:
    `- [ ] #task Design and configure "projects" in Obsidian! [p::2] [created::2026-06-11] ^prj` as the **first body
    line**, above `## Tasks` — the canonical placement. Its "Project Notes" section contains Bryan's original design
    notes for this very command.
  - `project.md` is the type-contract note (status values `wip | waiting | done | canceled`) and must be updated to
    document the `^prj` convention.
- Prior epic plan `sdd/epics/202606/obsidian_projects.md` (bob-cli-6): design contract for projects, task counting rules
  (open = not `x`/`X` and not `-`), vault-hygiene rules; its Out-of-Scope section explicitly deferred this command.
- `justfile` (`just all` = fmt + clippy + test; `install-smoke` enumerates every subcommand's `--help`), `tests/cli.rs`
  temp-vault integration-test patterns, `README.md` + `docs/` per-command documentation style.

## Design Contract (shared by all phases)

- **Project note** = a vault `.md` file whose _frontmatter_ `type` is `[[project]]` (accept quoted `"[[project]]"` and
  bare `[[project]]`). Scan the whole vault excluding `done/`, `.git/`, `.obsidian/`, `_templates/`, and `_generated/`.
- **Task** = a checkbox list line (`-`/`*`/`+` bullet, any indent) whose text matches the Tasks-plugin global filter
  `#task` (use the `bob-project-tasks` tag-boundary regex). **Open** = status mark not in {`x`, `X`, `-`} (so ` `, `/`,
  and `B` are open). Anywhere in the body, not just `## Tasks`.
- **`p` property** = inline Dataview field `[p::N]` (tolerate spaces: `[p:: N]`). A task without it is **implicitly
  P0**. The `^prj` task itself never counts toward the P0 search (it always carries `[p::2]`).
- **`^prj` task** = a task line whose trailing block ID is exactly `^prj`. At most one per note (multiple → per-file
  error, mirroring highlights' multiple-`^task` error). A `^prj` line that is not a valid open/done/canceled `#task`
  checkbox → per-file "malformed" error with a fix-it message naming the expected shape.
- **Status semantics** (the `^task`-in-`[[ref]]`-notes analogy): `^prj` mark `x`/`X` → `status: done`; `-` →
  `status: canceled`; ` `/`/`/`B` → no status change. Status writes are idempotent (already-matching status → no-op, no
  output line). One-directional: the task drives the frontmatter, never the reverse.
- **Scheduling rule**: for an _active_ project (status not `done`/`canceled` after any flip above) whose body contains
  **zero open P0 tasks** — vacuously true when there are no open tasks at all — and whose `^prj` task is open and has no
  `scheduled` field yet: insert `[scheduled::YYYY-mm-dd]` (today via `bob_env::current_datetime()`, local date)
  immediately **before** the `^prj` anchor, single-space separated. Never overwrite or remove an existing `scheduled`
  field.
- **Warnings (reported, never auto-fixed)**: active project with no `^prj` task; `^prj` still open while frontmatter
  status is already `done`/`canceled` (drift); `^prj` description still equal to the template placeholder
  `<short_project_completion_criteria_goes_here>`.
- **Terminal projects** (`done`/`canceled`): never warned about a missing `^prj` (move-done-tasks legitimately archives
  it) and never scheduled.
- **Lossless edits**: rewrite only the exact lines being changed (`status:` value swap in frontmatter, `[scheduled::]`
  insertion on the `^prj` line); never re-serialize/normalize frontmatter; preserve `zorg_*` metadata, block IDs, and
  all unrelated content byte-for-byte.
- **Vault hygiene** (Phase 3 and any real-vault verification): the vault is live and dirty under Obsidian Sync — check
  `git -C ~/bob status --short` first, re-read files immediately before writing, touch only listed files, commit only
  own files via `/sase_git_commit`.

## CLI Surface

```
bob projects                      Manage project notes via their ^prj tasks
  list  [-b|--bob-dir DIR]        Read-only overview of every project note
  sync  [-b|--bob-dir DIR] [-d|--dry-run]
                                  Sync project status and scheduling from ^prj tasks
```

- Registered in `src/runner.rs` `SUBCOMMANDS` between `pomodoro` and `tmux-pomodoro` (keeps the sorted-table test
  green); new module `src/native/projects.rs` + `NativeCommand::Projects`.
- Subcommand required (like `bob highlights`); options sorted alphabetically; every long option has a short alias; help
  template/styles match the `cli_styles()` palette; `--help` includes 2–3 worked examples.
- Exit codes: `0` on success (warnings included); `1` when any per-file error (multiple/malformed `^prj`, unreadable
  file, write failure) occurred — errors are reported per file and never abort the rest of the scan.

## Output Design (Phase agents polish; this is the direction)

Color only when stdout is a TTY (`std::io::IsTerminal`) and `NO_COLOR` is unset — plain, aligned ASCII otherwise (same
spirit as the `justfile` banners; no new dependencies needed, manual ANSI is fine).

`bob projects list` — one aligned row per project, active first (then waiting, done, canceled), with a one-line header
summary:

```
Projects · 6 active · 0 waiting · 0 done · 2 canceled

  PROJECT                  STATUS  OPEN  P0  ^PRJ
  bob_projects             wip        2   1  ○ open
  bob                      wip       17   0  📅 2026-06-11
  gkeep_gdocs_inbox_dump   wip       45  45  ○ open
  sase_blog                wip        3   3  ⚠ missing
  ...
```

(`^PRJ` column states: `○ open`, `📅 <scheduled date>`, `✓ done`, `✕ canceled`, `⚠ missing`, `⚠ placeholder`. Status
colors: `wip` yellow, `waiting` blue, `done` green, `canceled` dim. Project names cyan.)

`bob projects sync` — one line per action/warning only (quiet on healthy projects), then always a one-line summary:

```
  ✓ sase_blog       status: wip → done            ^prj task checked
  📅 bob            scheduled ^prj for 2026-06-11  no open P0 tasks
  ⚠ outlive         active project has no ^prj task

11 projects · 1 status updated · 1 scheduled · 1 warning
```

`--dry-run` uses the same lines with a leading `[dry-run]` marker / "would …" phrasing and writes nothing.

---

## Phase 1 — `projects` Module, Vault Scanner & `bob projects list`

**Purpose**: the read-only foundation — project discovery, parsing model, and the `list` overview.

### Scope (bob-cli repo)

- Run the `/sase_memory_read` procedure on `memory/long/cli_rules.md` before touching the CLI.
- `src/native/projects.rs`: clap command (subcommand-required, `list` wired; `sync` arrives in Phase 2), vault scanner
  per the Design Contract, and the parsed-project model: relative path/name, `status`, `^prj` line state (missing / open
  / done / canceled / malformed, plus description, `p`, `scheduled`, placeholder detection), open-task and open-P0
  counts. Parsing is self-contained (own checkbox/tag/inline-field/block-id helpers + unit tests) — do **not** refactor
  `highlights_ref`.
- Register in `src/native.rs` + `src/runner.rs` (alphabetical slot, `about` string, `AFTER_HELP` example such as
  `bob projects sync --dry-run`).
- `bob projects list` output per the Output Design.
- `justfile install-smoke` + README smoke list gain `bob projects --help`; README gets a short `bob projects` section
  (usage block; full prose lands with Phase 2's docs).
- Tests: unit tests for the parsers/classification; `tests/cli.rs` integration tests against temp vaults (fixture notes
  covering quoted/bare `type`, excluded directories, `^prj` variants, open-mark variants ` `/`/`/`B`, `[p:: 2]` spacing)
  plus `--help` content checks.

### Verification

- `just all` green; `bob projects list` against a fixture vault renders the documented layout (TTY and piped modes).
- Manual smoke against the real vault (read-only): `cargo run -- projects list` shows the 15 current projects with
  plausible counts.
- `/sase_git_commit` of only this phase's files.

---

## Phase 2 — `bob projects sync` Mutation Engine

**Purpose**: the reconciliation behavior that makes `^prj` the project lifecycle control.

### Scope (bob-cli repo)

- Run the `/sase_memory_read` procedure on `memory/long/cli_rules.md` first.
- `sync` subcommand in `src/native/projects.rs` implementing the Design Contract exactly: status flips (`x`/`X` →
  `done`, `-` → `canceled`) via lossless `status:` line replacement (append a `status:` line after `type:` if a project
  somehow lacks one), `[scheduled::YYYY-mm-dd]` insertion before the `^prj` anchor, warnings, per-file errors,
  `-d|--dry-run`, exit codes, and Output-Design rendering.
- "Today" comes from `bob_env::current_datetime()` so tests pin `BOB_NOW`.
- Tests (unit + `tests/cli.rs` temp vaults): each flip; canceled flip; no-op when status already matches; scheduling on
  zero-open-P0 (including the zero-open-tasks vacuous case); no scheduling when a P0 exists / `scheduled` already
  present / project terminal / `^prj` checked; `^prj`-excluded-from-P0 check; drift + missing + placeholder warnings;
  multiple-`^prj` and malformed-`^prj` errors (exit 1, other files still processed); dry-run leaves bytes untouched;
  **idempotency** (second sync run = zero actions); unrelated file content preserved byte-for-byte.
- Docs: `docs/projects.md` (style of `docs/highlights-ref-sync.md`) covering the contract, the `^task`/`[[ref]]`
  analogy, and worked examples; README section completed and linked.

### Verification

- `just all` green; `cargo run -- projects sync --dry-run` against the real vault reports plausible pending actions and
  **writes nothing** (`git -C ~/bob status` unchanged).
- `/sase_git_commit` of only this phase's files.

---

## Phase 3 — Template Update, Vault Migration & Epic Close

**Purpose**: roll the convention out to the live vault and finish the epic. All edits in `~/bob` (vault repo). Re-verify
every classification against live file contents at execution time — the vault changes daily; the tables below are the
expected starting point, and deviations are fine with rationale in the commit message.

### Scope (vault repo)

1. **`_templates/new_project.md`**: insert the task line as the **first body line** (immediately after the closing
   `---`, above `# <% tp.file.title %>`), exactly:

   ```
   - [ ] #task <short_project_completion_criteria_goes_here> [p::2] ^prj
   ```

   The placeholder is deliberate — `bob projects sync` warns until Bryan replaces it, and Phase 2's placeholder warning
   depends on this exact text.

2. **Retire legacy projects** — delete the `type:` frontmatter line (plus machine-maintained `task_count:` /
   `open_task_count:` lines, which the `bob-project-tasks` plugin would strip anyway once the note is no longer a
   project) from project notes with **zero open `#task` tasks**. Keep `status` and everything else untouched. Expected
   set (9):

   | Note                                                                                                                                           | Evidence                                                                              |
   | ---------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
   | `balance_coupling.md`, `cat_theory_for_devs.md`, `clean_arch.md`, `how_to_read_a_book.md`, `soft_arch_hard_parts.md`, `think_fast_and_slow.md` | book projects; raw `- [ ]` checklists without `#task` — invisible to the Tasks plugin |
   | `outlive.md`                                                                                                                                   | zorg-generated stub pointing at ref notes; 0 `#task` tasks                            |
   | `prj_yserve.md`, `prj_zorg.md`                                                                                                                 | zorg-generated, already `status: canceled`, 0 `#task` tasks                           |

3. **Add `^prj` tasks to the remaining live projects** — first body line, format
   `- [ ] #task <criteria> [p::2] [created::<today>] ^prj` (matching Bryan's prototype; `[created::]` because the Tasks
   plugin only auto-stamps tasks created in-app). The agent reads each note and crafts a _short, concrete,
   outcome-shaped_ completion criteria. Proposed starting points (refine against content):

   | Note                        | Proposed criteria                                                                    |
   | --------------------------- | ------------------------------------------------------------------------------------ |
   | `bob.md`                    | Bob vault tooling is feature-complete and boring — no open improvement tasks remain! |
   | `gkeep_gdocs_inbox_dump.md` | Empty this Google Keep inbox dump — every task triaged or done!                      |
   | `sase_blog.md`              | Publish the full sase.sh blog series!                                                |
   | `sase_install.md`           | `sase install` works end-to-end on a fresh machine!                                  |
   | `sase_version.md`           | SASE versioning scheme designed and released!                                        |

   `bob_projects.md` already has a conforming `^prj` — verify, do not change.

4. **Update `~/bob/project.md`** (type-contract note): document the `^prj` completion-criteria task (placement, format,
   that checking/canceling it drives `status` via `bob projects sync`, and the no-open-P0 ⇒ scheduled-today rule).

5. **End-to-end validation** (the feature's first real run):
   - `bob projects list` — 6 projects, statuses/counts correct.
   - `bob projects sync --dry-run` — expected: exactly one pending action (`bob.md` gets `[scheduled::<today>]`; its
     open tasks all carry `[p::N]`), zero status flips, zero warnings.
   - Run `bob projects sync` for real; re-run → zero actions (idempotent); spot-check `bob.md`'s `^prj` line.

6. **Close out**: `/sase_git_commit` the vault changes; close this epic's bead (`sase bead close <epic-bead-id>`) after
   confirming all child beads are closed; set the epic plan file's frontmatter `status` to `done`.

### Verification

- Frontmatter of every touched note still YAML-parses; `git -C ~/bob diff` touches only the listed files.
- Re-run the project scan: exactly the 6 live projects remain typed `[[project]]`, each with a well-formed `^prj`.
- Manual acceptance for Bryan: create a scratch project via `Ctrl+Shift+N` (template task appears, sync warns about the
  placeholder); check off a project's `^prj` in Obsidian and run `bob projects sync` (status flips to `done`,
  `projects.base` moves it to Closed).

---

## Risks

- **Live, dirty vault** (Obsidian Sync): Phase 3 and real-vault smoke tests re-check `git status` before/after and never
  stage unrelated files.
- **`move-done-tasks` interplay**: checked `^prj` lines are eventually archived into `done/` (block-ID dedup handles
  `^prj` collisions across archives). The terminal-project exemption in the Design Contract is what keeps `sync` quiet
  afterward — tests must cover it.
- **`status` property is shared** with the ref-reading pipeline (`unread`/`read`/…): the scanner keys strictly off
  `type: [[project]]`, so the value sets cannot mix.
- **Tasks-plugin custom marks**: ` `/`/`/`B` are open, only `x`/`X`/`-` are terminal — getting this wrong either
  prematurely closes projects or misses P0s; it's pinned in the Design Contract and the Phase 1/2 test matrices.
- **`bob.md` self-reference**: `bob.md` is itself a live project whose `^prj` gets scheduled by the first real sync —
  expected, called out in Phase 3 so the agent doesn't mistake it for a bug.
- **Counts drift between planning and execution**: every Phase 3 table is re-verified live; the rule (zero open `#task`
  tasks ⇒ legacy) is authoritative, not the file lists.

## Out of Scope (future work, from Bryan's design notes in `bob_projects.md`)

- Crontab / `bob nightly` integration of `bob projects sync`.
- Reverse sync (frontmatter `status` → `^prj` checkbox) and removing stale `[scheduled::]` fields when a P0 task later
  appears.
- Required-frontmatter validation (`parent`, etc.) and `bob projects` participation in `bob bulk-git-commit`.
