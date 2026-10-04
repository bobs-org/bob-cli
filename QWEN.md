# bob-cli - Agent Instructions

## 1. Core Memory

The following memories contain core (always loaded) context:

### 1.1 SASE = Structured Agentic Software Engineering (sase)

#### 1.1.1 SASE Memory

SASE memory is this project's durable agent context: Markdown notes under `sase/memory/`
that render into this file. A note's kind — flat note or memory web — and a flat note's
`type:` frontmatter decide how it reaches you.

- **Core memory** (`type: core`) is inlined here and into every provider instruction
  shim, so it is always in your context and is paid for on every turn.
- **Reference memory** (`type: reference`) is not inlined. Only its one-line description
  is listed here; read the body on demand with your `/sase_memory_read` skill, never by
  opening the file directly.
- **Memory webs** are keyed collections: a flat descriptor note (`sase/memory/<web>.md`)
  plus a sibling directory of strand files (`sase/memory/<web>/<slug>.md`). A web's
  descriptor is always inlined here; a strand body never is — read strands on demand
  with your `/sase_memory_read` skill (`sase memory read <web>:<keyword>`, for example
  `glossary:stitch`).

Memory files are not ordinary files: before you create, edit, or delete any of them — or
propose a plan that would — use your `/sase_memory_write` skill.

#### 1.1.2 Ephemeral `bob-cli_<N>` Workspace Directories

SASE runs agents (like you) from ephemeral workspace directories, which are full clones
of the bob-cli repo. These directories are named `bob-cli_<N>` where `<N>` is some
integer. You need to be mindful not to run commands outside of these workspace
directories.

IMPORTANT: Do NOT mention your workspace directory (or any sibling workspace directory)
in any plan files that you generate using your `/sase_plan` skill. The agent(s) that
implement the plan might not run in the same workspace directory as you!

#### 1.1.3 Repositories

Configured linked and sidecar repositories associated with this project:

- `bob-plugins`: Source-of-truth monorepo for Bryan's custom Bob Obsidian plugins,
  deployed to the vault via `bob plugins sync`. You should NOT edit these plugins
  directly in the ~/bob/ directory, as they will be overwritten on the next sync.
  Instead, make changes to this linked repo and, when done, run the `bob plugins sync`
  command to deploy them to the ~/bob/ directory.
- `bob-mac-capture`: Native macOS menu-bar frontend for Bob capture. It delegates
  capture grammar, completion, live preview, and vault mutation to bob-cli's versioned
  `bob` subprocess/JSON interfaces, so coordinate capture-contract changes across both
  repositories.
- `bob-cli--research`: Durable SASE research reports and generated media.

When you need to read or modify files in any repository other than your own workspace
checkout, agents MUST use your `/sase_repo` skill first. This includes configured linked
repos and sidecars, another SASE project's repo, and any GitHub repo not linked to the
current project. Open different-project and unlinked GitHub repos as external repos
through the skill. Use the path it prints as the only path for reads and writes.

This rule applies regardless of transport. Fetching a repository's files or history over
the web — github.com file/blob/raw URLs, raw.githubusercontent.com, repo tarballs, or
GitHub-API/`gh` file-content reads — counts as reading that repo: open it with
`/sase_repo` (unlinked GitHub repos open as external repos) and read the local checkout
instead. Web tools remain appropriate only for content a checkout does not contain, such
as blog posts, docs sites, and GitHub issue/PR discussions.

**IMPORTANT**: The `sase artifact read <ref> "<reason>"` command MUST be used to read
artifacts (so the reads are audited) from sidecar repos. Do NOT read sidecar artifact
files directly or locate, clone, or web-fetch another repo's contents any other way than
by using `/sase_repo` or `sase artifact read`!

#### 1.1.4 SASE Final Declaration

Before any normal response that ends this SASE provider turn, use your `/sase_final`
skill as the last action. This includes a final answer and an incomplete-status
response; an unfinished turn still declares so its work is committed. Never end a turn
to wait for a command or to resume later: nothing can wake you, so hand long commands to
`/sase_monitor` before starting them. Only a successfully executed plan, monitor, pipe,
or questions handoff is exempt, because those commands terminate the runner
mechanically.

## 2. Reference Memory

The below files contain detailed reference material. When working in their domain, you
MUST use your `/sase_memory_read` skill to review their contents. Do not read canonical
memory files directly.

1. **`sase/memory/cli_rules.md`** - Read anytime new CLI subcommands or options are
   added.
2. **`sase/memory/sase_artifacts.md`** - Read before creating, consuming, resolving,
   linking, or managing retention for SASE artifact references and indexed files.
3. **`sase/memory/sase_beads.md`** - Read before creating, updating, closing, or
   querying sase beads — bead types and tiers, the status lifecycle agents must never
   hand-edit, task-bead triage, phase-bead description prefixes, and non-cascading
   close, resolution, and note semantics.

## 3. Memory Webs

Each memory web below is a keyed collection. Its descriptor is always loaded, but a
strand's body is not: read strands on demand with your `/sase_memory_read` skill, for
example `sase memory read glossary:stitch -r "<why>"`.

### 3.1 Decisions (decisions)

Accepted architecture and policy decisions spanning bob-cli, bob-plugins, Bob Mac
Capture, and the Bob vault. Each roster summary is a rule to follow as written. Before
changing behavior a record governs, or proposing one of its rejected alternatives, read
it with `sase memory read decisions:<keyword> -r "<why>"`; each record states where it
applies, the claim, why it beat the credible alternatives, what it costs, and what would
reopen it. A record is not a design doc, runbook, command contract, or keymap — those
live in `docs/` and each repo's README. Records cite checkable evidence (a commit, doc,
plan, or research ref); when no source records the reason, ask Bryan rather than infer
one. A record is immutable once accepted: if course changes, write a new record and mark
the old one with `metadata.status` plus `superseded_by` and a `[[...]]` back-link, never
edited in place.

1. **#now Is A User-Owned Weekly Bet, Never A Status** (`now-tag-is-user-owned`) -
   _[superseded by `task-lanes-are-sticky`, `today-is-read-from-the-ledger`]_ Only
   Bryan's explicit gestures add or remove #now; no automation infers, adds, or strips
   it. It never changes task status or feeds Next, and it stays a tag, never an inline
   field.
2. **Active Task Statuses Are Derived, Not Authored** (`task-status-is-derived`) -
   _[partly superseded by `task-lanes-are-sticky`, `task-deps-are-depends-on-links`]_
   Today's Pomodoro ledger drives Next and In Progress; open dependencies and future
   scheduled dates drive Blocked. bob task-status-hooks reconciles them, so writers
   change those inputs, never just the checkbox.
3. **Bob Mac Capture Is A Thin Client Of bob** (`mac-capture-is-a-thin-client`) - Bob
   Mac Capture never parses capture grammar, computes previews, or writes the vault; it
   runs bob, renders the spans, candidates, and previews bob returns, and submits each
   draft as one bob capture call.
4. **Decay Decisions Are Available Immediately**
   (`decay-decisions-are-available-immediately`) - Gesture-triggered approved-decay
   cards are available as soon as compatible plugins are installed; there is no calendar
   gate, replacement date, or counting-only period.
5. **Next And Pending Are Sticky Lanes; Only Blocked Is Derived**
   (`task-lanes-are-sticky`) - Linking raises Ready to Next and an =x close sets In
   Progress (PENDING); no unlink, hooks run, or capture drop lowers them; only Alt+N
   release returns a task to Ready; Blocked stays derived.
6. **Note Ready Cap Counts The Lane** (`note-ready-cap-counts-the-lane`) - The per-note
   soft cap (plan.max_ready_per_note, default 5; ready_cap: N|off) counts each
   area/project note's whole Ready lane by residence, whatever its freshness.
7. **READY Is Freshness-Gated With NEW and ROTTEN Review**
   (`ready-is-freshness-gated`) - _[partly superseded by
   `note-ready-cap-counts-the-lane`, `review-walk-is-tiered`,
   `rotten-keeps-use-priority-decay`]_ READY is the freshness-gated confirmed/exempt
   backlog (visible TODO pool minus NEW and ROTTEN buckets) with TODAY → NEW → PENDING →
   NEXT → READY sections and NEW/PENDING/NEXT/READY/BLOCKED/ROTTEN/TODAY chips; review
   clears NEW then ROTTEN; no tags, fields, or status changes store review state.
8. **Review Walk Is Tiered With Daily Lane Review** (`review-walk-is-tiered`) - The ]s
   walk visits one shared queue in explicit tiers NEW → PENDING → NEXT → RETURNED →
   ROTTEN; Pending and Next tasks come due for daily review under pending_interval /
   next_interval (default 1, false walks that lane off); tiers never feed buckets or
   chips; upkeep outside the lanes counts the budget; stamps stay and the seed never
   re-runs.
9. **Rotten Keeps Decay Through The Priority Ladder**
   (`rotten-keeps-use-priority-decay`) - _[partly superseded by
   `decay-decisions-are-available-immediately`]_ Repeated due-Ready keeps earn an
   explicit approved decision that enters the existing priority ladder; nothing decays
   silently and freshness itself still never changes priority or schedule.
10. **Task Dependencies Are Links On One Depends-On Line**
    (`task-deps-are-depends-on-links`) - A task's prerequisites live as plain task
    dependency links on one managed Depends-On first-child line; that line is the source
    of truth and the [dependsOn::] / [id::] fields are derived from it.
11. **Today Is Read From The Ledger, Never Written To Tasks**
    (`today-is-read-from-the-ledger`) - _[partly superseded by
    `ready-is-freshness-gated`]_ Today is the open tasks with a dedicated Task Link
    under today's open Pomodoros, computed at read time by bob plan and
    bob-ledger-tools; never a tag, task-line field, or file-path filter; #now is
    retired.

### 3.2 Glossary Terms (glossary)

Run `sase memory read glossary:<term> [<term> ...] -r "<why>"` before relying on any of
these SASE terms; it prints each term's definition plus every term those definitions
depend on. Pass every term you need in one command — one batched read costs far fewer
tokens than one read per term, because terms shared between definitions are printed
once. Terms are separated by semicolons; aliases follow in parentheses.

**GLOSSARY TERMS:** Keep Streak (keeps); Pomodoro; Project Note (prj note); Project Task
(prj task); Reference Note (ref note); Reference Task (ref task); Schedule Log; Task
Dependency Link (task dep link, dep link); Task Freshness (freshness); Task Link (task
block link); Work Log

### 3.3 Task Bead Types (task_types)

Every task bead can carry a `task_type` drawn from this project's catalog.
`sase bead task-type list` always shows the live catalog; read
`sase memory read task_types:<slug> -r "<why>"` for one generated type in full. This
note is the generated, always-current snapshot of the agent-creatable types below.

1. **Bug** (`bug`) - A defect an agent found while doing unrelated work, not an external
   tracker bug.
2. **CI failure** (`ci`) - A confirmed true test or lint failure you did not cause, not
   a flake.
3. **Feature** (`feature`) - An out-of-scope product or tooling idea that should not
   become a wish list.
4. **Flaky test** (`flake`) - A test that fails and then passes on an unchanged tree.
5. **Memory** (`memory`) - A sase memory note or skill that is out of date.

#### 3.3.1 File Discovered Work As Task Beads

Unless your prompt explicitly forbids creating beads (epic phase workers, for example,
must record `PROPOSED FOLLOW-UP:` notes on their own bead instead), you can and SHOULD
capture discovered follow-up work as sase task beads. Before creating any task bead, you
MUST use `/sase_new_task`.
