# bob-cli - Agent Instructions

> This file is generated from `sase/memory/` by `sase memory init`; edit the notes, not
> this file. SASE-launched agents get a per-invocation bundle from the same notes plus
> home memory and a provider directive. Preview it with `sase instructions render`; see
> what an agent received with `sase instructions show NAME`.

## 1. Core Memory

The following memories contain core (always loaded) context:

### 1.1 SASE = Structured Agentic Software Engineering (sase)

#### 1.1.1 SASE Memory

SASE memory is this project's durable agent context: Markdown notes under `sase/memory/`
that render into your agent instructions (this project's `AGENTS.md` and each SASE
agent's per-invocation bundle). A note's kind — flat note or memory web — and a flat
note's `type:` frontmatter decide how it reaches you.

- **Core memory** (`type: core`) is inlined into your agent instructions (this project's
  `AGENTS.md` and each SASE agent's per-invocation bundle), so it is always in your
  context and is paid for on every turn.
- **Reference memory** (`type: reference`) is not inlined. Only its one-line description
  is listed in your agent instructions; read the body on demand with your
  `/sase_memory_read` skill, never by opening the file directly.
- **Memory webs** are keyed collections: a flat descriptor note (`sase/memory/<web>.md`)
  plus a sibling directory of strand files (`sase/memory/<web>/<slug>.md`). A web's
  descriptor is always inlined into your agent instructions; its roster is too unless
  the web loads its roster on demand (read with `sase memory read roster:<web>`); a
  strand body never is — read strands on demand with your `/sase_memory_read` skill
  (`sase memory read <web>:<keyword>`, for example `glossary:stitch`).

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
  repositories. GitHub Actions is the only gate for the macOS-only targets, so agents
  watch the exact-SHA macOS run to green with `/sase_monitor` after every push, or gate
  first with `scripts/dev/macos-ci-check`.
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

Only the root SASE agent — the provider turn SASE launched — submits the final
declaration; if you were spawned or forked as a helper (a native subagent), return your
result to your parent instead. Before any normal response that ends this SASE provider
turn, use your `/sase_final` skill as the last action. This includes a final answer and
an incomplete-status response; an unfinished turn still declares so its work is
committed. Never end a turn to wait for a command or to resume later: nothing can wake
you, so hand long commands to `/sase_monitor` before starting them. Every root turn ends
through `/sase_final`, including handoff turns: a staged handoff is adopted by the host
after finalizers settle.

### 1.2 Gotchas (gotchas)

bob-cli ships only through the scheduled release train: at most one version per New York
day after full CI on the exact SHA, with no release PR. Never edit `Cargo.toml` versions
or `CHANGELOG.md`; mark CLI or JSON contract breaks with `!`. See
[[decisions/bob-cli-releases-ride-a-daily-train]].

## 2. Reference Memory

The below files contain detailed reference material. When working in their domain, you
MUST use your `/sase_memory_read` skill to review their contents. Do not read canonical
memory files directly.

1. **`sase/memory/cli_rules.md`** - Read anytime new CLI subcommands or options are
   added.
2. **`sase/memory/docs.md`** - Read before changing docs/, mkdocs.yml, tools/docs,
   README front doors, or user-visible CLI help.
3. **`sase/memory/sase_artifacts.md`** - Read before creating, consuming, resolving,
   linking, or managing retention for SASE artifact references and indexed files.
4. **`sase/memory/sase_beads.md`** - Read before creating, updating, closing, or
   querying sase beads — bead types and tiers, the status lifecycle agents must never
   hand-edit, task-bead triage, phase-bead description prefixes, and non-cascading
   close, resolution, and note semantics.

## 3. Memory Webs

Each memory web below is a keyed collection. Its descriptor is always loaded with its
roster unless the descriptor says the roster loads on demand; read those with
`sase memory read roster:<web>`. A strand's body is not: read strands on demand with
your `/sase_memory_read` skill, for example
`sase memory read glossary:stitch -r "<why>"`.

### 3.1 Decisions (decisions)

Accepted architecture and policy decisions spanning bob-cli, bob-plugins, Bob Mac
Capture, and the Bob vault. Records are rules once read: before writing or revising a
SASE plan, read the roster with `sase memory read roster:decisions -r "<why>"` and cite
each governing decision as `decisions:<keyword>`; before changing governed behavior
directly — capture, walk/review, tasks/lanes/decay, gcal/meetings, refs,
themes/relations, releases, or Mac Capture — consult the roster first when no plan cites
the decision. Each record states where it applies, the claim, why it beat the credible
alternatives, what it costs, and what would reopen it. A record is not a design doc,
runbook, command contract, or keymap — those live in `docs/` and each repo's README.
Records cite checkable evidence (a commit, doc, plan, or research ref); when no source
records the reason, ask Bryan rather than infer one. A record is immutable once
accepted: if course changes, write a new record and mark the old one with
`metadata.status` plus `superseded_by` and a `[[...]]` back-link, never edited in place.

**Roster on demand.** This web's 82 decisions are not listed here. You MUST read the
DECISIONS roster with `sase memory read roster:decisions -r "<why>"`:

- before writing or revising a SASE plan (any agent using `/sase_plan`); cite each
  decision that governs the plan as `decisions:<keyword>`
- when performing research, to find decisions related to the question before
  recommending anything
- before changing governed behavior when no plan cites the decision: capture,
  walk/review, tasks/lanes/decay, gcal/meetings, refs, themes/relations, releases, or
  Mac Capture

Read one decision with `sase memory read decisions:<keyword> -r "<why>"`.

### 3.2 Glossary Terms (glossary)

Run `sase memory read glossary:<term> [<term> ...] -r "<why>"` before relying on any of
these SASE terms; it prints each term's definition plus every term those definitions
depend on. Pass every term you need in one command — one batched read costs far fewer
tokens than one read per term, because terms shared between definitions are printed
once. Terms are separated by semicolons; aliases follow in parentheses.

**GLOSSARY TERMS:** Album Card; Album Note; Annotation Section (section tag, section
card); Area Card; Area Note; Bank Badge (bank chip, daily bank chip); Bank Card
(bob-bank, setup panel); Bank Closing (period close, period closing balance, closing
balance); Bank Month Note (bank month, month note); Bank Row (bank row, transaction
row); Bulk Capture (multi-item capture, batch capture); Capture; Capture Alias (sigil
alias); Capture Panel; Command Chip (run chip, bob-run); Command Mode (Bob Command Mode,
command sheet); Copy Chip (chip, copy card fence, bob-copy); Daily Headline (day
headline) [superseded]; Day and Period Card (intentions card, intention card, day card,
period card); Day Strip (day stats); Event Ack (ack, unacked, event acknowledgement,
gcal ack); Event Task (event row, calendar task); Gmail Task (gmail task, email task);
Inbox File (inbox note); Intention (daily intentions, weekly intentions); Keep Streak
(keeps); Leaf Project; Mac Menu Bar Pomodoro Indicator (mac pom); Merged Pomodoro
(merged pomodoro, composite pomodoro, base type); Morning GTD Review Footer (review
footer); Notebook (notebook note); Off-Clock Pomodoro (off-clock session, untracked
pomodoro, untracked session); Period Note (weekly note, quarterly note, yearly note,
all\_time); Pomodoro; Project Card; Project Note (prj note); Project Note Capture
(project capture, project-note capture); Project Task (prj task); Property Card (card
family); Prose Note (prose note, article note); Record Card (record sheet); Record Note;
Ref Card (reference card, ref title card); Ref Panel (Bob Refs, refs panel); Reference
Note (ref note); Reference Task (ref task); Root Parent; Rotten Log (keep log); Schedule
Log; Stats Dash (dash\_stats); Stats Strip (period strip, week strip, quarter strip,
year strip); Successor Offer; Task Decay (decay, decay ladder, decay card, step down);
Task Dependency Link (task dep link, dep link); Task Drawer (drawer); Task Freshness
(freshness); Task Link (task block link); Task Mention (mention, text mention); Task
Merge; Task Relation (relation, LINKS line, LINKED FROM line); Task Requirement (req,
task req); Task Split; Task Successor (successor); Task Type (type, theme, pomodoro
theme, pomodoro-theme); Top-Level Project (top project); Walker Footer (walk footer,
lane footer, links footer, bank footer); Work Log

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
