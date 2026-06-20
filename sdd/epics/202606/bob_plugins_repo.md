---
create_time: 2026-06-20 11:00:33
bead_id: bob-cli-8
tier: epic
status: wip
prompt: sdd/prompts/202606/bob_plugins_repo.md
---
# Plan: Migrate Bob Obsidian plugins to `bbugyi200/bob-plugins` + add `bob plugins`

## Goal

Stand up the new `bbugyi200/bob-plugins` GitHub repo as the source of truth for Bryan's six custom Obsidian plugins, and
add a `bob plugins` command to bob-cli to manage them (list + sync to the vault). Wire the new repo into this repo as a
SASE sibling. The design of `bob plugins list` is ours to lead and must look beautiful.

Inspiration / prior art: `sdd/research/202606/bob_obsidian_plugins_repo_consolidated.md`.

## Background & current state (verified 2026-06-20)

- **Six custom plugins** live in `~/bob/.obsidian/plugins/<id>/`, all authored by `Bryan`, all enabled in
  `~/bob/.obsidian/community-plugins.json`, all plain CommonJS (`manifest.json` + `main.js`, plus `styles.css` for
  `bob-navigation-hotkeys`). No build/TS/bundler. IDs: `block-id-prompt`, `bob-ledger-tools`, `bob-navigation-hotkeys`,
  `bob-project-tasks`, `bob-vim-surround` (v1.2.0; others v1.0.0), `task-status-cycler`.
- **Migration blocker:** `~/bob/.obsidian/plugins/bob-vim-surround/main.js` is modified (uncommitted) in the vault
  working tree. The on-disk working-tree copy is the _latest_ source (recent ys/cs/ds fixes) and must be the version
  that lands in the repo. Do not lose it.
- **Target repo** `~/projects/github/bbugyi200/bob-plugins/` exists, remote `git@github.com:bbugyi200/bob-plugins.git`
  is linked, one commit (`chore: First Commit`), only an empty `README.md`.
- **bob-cli is a Rust CLI.** Subcommands are a sorted table `SUBCOMMANDS` in `src/runner.rs` → `NativeCommand` enum in
  `src/native.rs` → a module under `src/native/`. `src/native/projects.rs` is the closest template: a native command
  with `list`/`sync` subcommands, clap argument building, and colored output via `src/native/style.rs` (`Styler`:
  cyan/green/yellow/blue/red/dim, `pad_right`, `display_width`). Vault path resolves via `src/native/env.rs::bob_dir()`
  (`BOB_DIR` or `~/bob`, with `expand_tilde`).
- **CLI conventions** (`memory/cli_rules.md`): excellent `-h/--help`; subcommands and options sorted alphabetically;
  every public long option gets a short alias; prefer beautiful colored output. There is a unit test
  (`subcommands_are_sorted_alphabetically`) that enforces the `SUBCOMMANDS` order, and `build_cli().debug_assert()`
  validates help rendering.
- **Surfaces that list every subcommand** and must be kept in sync when adding `plugins`: `src/runner.rs`
  (`SUBCOMMANDS` + `AFTER_HELP` examples), `src/native.rs` (`NativeCommand`), `justfile` `install-smoke`, `README.md`,
  `docs/`, and `tests/cli.rs`.
- **Sibling-repo resolution** (`sase` `src/sase/sibling_repos.py`): `path` entries are
  `expanduser`/`expandvars`-expanded; relative paths resolve against the _primary_ workspace dir. The sase project uses
  relative `../sase-core` because its canonical repo and siblings share a parent. bob-cli's ephemeral workspaces
  (`~/.local/state/sase/workspaces/...`) are **not** adjacent to its canonical repo
  (`~/projects/github/bbugyi200/bob-cli`), so use an absolute `~`-prefixed path to avoid ambiguity. A non-existent
  primary path is silently skipped with a warning, so resolution must be verified.

## Design decisions (lead)

### `bob-plugins` repo layout (per research; plain JS, no build)

```
bob-plugins/
  README.md            # great README (see Phase 1)
  LICENSE
  .gitignore           # node_modules, .DS_Store, *.log
  package.json         # repo tooling only (NOT bundling) — npm run validate
  scripts/
    validate-manifests.mjs   # manifest parses; required fields; id==folder; semver; main.js parses under node
  plugins/
    block-id-prompt/{manifest.json,main.js}
    bob-ledger-tools/{manifest.json,main.js}
    bob-navigation-hotkeys/{manifest.json,main.js,styles.css}
    bob-project-tasks/{manifest.json,main.js}
    bob-vim-surround/{manifest.json,main.js}
    task-status-cycler/{manifest.json,main.js}
```

Keep per-plugin independent versions (no lockstep). No TypeScript, no shared package extraction in this migration.

### `bob plugins` command shape

- `bob plugins` with **no subcommand → runs `list`** (the default). Implement by detecting `None` from
  `matches.subcommand()` and dispatching to `run_list` with defaults (unlike `projects`, which is
  `subcommand_required(true)`).
- Subcommands (declared sorted): `list`, `sync`.
- Source of truth for both is the **repo** (`<repo>/plugins/<id>/manifest.json`), cross-referenced against the **vault**
  for install/enabled/sync state.
- Repo path resolution: `-r/--repo <DIR>` → `BOB_PLUGINS_DIR` env → default `~/projects/github/bbugyi200/bob-plugins`.
  Vault path: reuse `bob_dir()` (`-b/--bob-dir`, `BOB_DIR`, `~/bob`).
- Honor `cli_rules.md`: short alias for every long option, alphabetical help, `Styler`-based color with
  `NO_COLOR`/non-TTY fallback.

### `bob plugins list` (default) — beautiful, value-first

Render a colored, aligned table built from the repo manifests and annotated with live vault state. Recommended columns
(implementer may refine spacing/glyphs but keep the information set and the polish):

| Column        | Source                                                                                                                 | Styling                          |
| ------------- | ---------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| `PLUGIN` (id) | repo folder / manifest `id`                                                                                            | cyan bold                        |
| `VERSION`     | repo manifest `version`                                                                                                | plain / dim                      |
| `SYNC`        | repo files vs vault files (`manifest.json`/`main.js`/`styles.css` byte-compare) → `✓ synced` / `⚠ drift` / `✗ missing` | green / yellow / red             |
| `VAULT`       | `community-plugins.json` → `enabled` / `disabled`, or `not installed`                                                  | green / dim / red                |
| `DESCRIPTION` | repo manifest `description`                                                                                            | dim, truncated to terminal width |

Plus:

- A header line: repo path + plugin count (e.g. `Bob Plugins · 6 · <repo>`).
- A footer summary: counts (`N synced · M drift · K not installed`); optionally total `main.js` LOC.
- `-f/--format <table|json>` (default `table`; `json` mirrors `capture_targets`'s machine-readable mode for scripting).
- Exit non-zero only on real errors (e.g. repo unreadable), not on drift.

### `bob plugins sync` — repo → vault deploy

- Copies only `manifest.json`, `main.js`, and `styles.css` (when present) from `<repo>/plugins/<id>/` into
  `~/bob/.obsidian/plugins/<id>/`. Never touches `data.json` or other runtime/settings files.
- `-p/--plugin <id>` to sync one plugin; default syncs all six.
- `-d/--dry-run` previews; `-F/--force` required to overwrite a vault plugin file that is **dirty in the vault git
  repo** (default: refuse + warn, so uncommitted vault edits like the current `bob-vim-surround/main.js` are never
  clobbered silently).
- Colored per-plugin action report using `Styler::success_prefix(dry_run)` / `warning_prefix()` (same idiom as existing
  commands).

## Out of scope / explicit non-goals (this migration)

- TypeScript, bundling/esbuild, or shared-helper extraction.
- Git history preservation for the plugin folders (snapshot import is fine; see Open Questions if history is wanted).
- `git rm --cached` of the six folders from the **vault** to make the repo the sole source of truth. This has
  multi-machine implications and is left as a deliberate follow-up decision (see Open Questions). Until then the vault
  keeps its working copies and `bob plugins sync` is the deploy path.
- Committing unrelated dirty vault notes, or publishing any plugin via BRAT / the official community directory.

## Phases

Each phase is executed by a **distinct agent instance** and ends by committing its own changes (via the
`sase_git_commit` skill / SASE finalizer). Phases are ordered; cross-phase dependencies are called out.

### Phase 1 — Populate & document the `bob-plugins` repo

_Repo: `~/projects/github/bbugyi200/bob-plugins/`. Depends on: nothing._

1. Capture the **on-disk** (working-tree) copies of the six plugin folders from `~/bob/.obsidian/plugins/<id>/` — this
   captures the latest `bob-vim-surround/main.js`. Copy only `manifest.json`, `main.js`, and `styles.css` (navigation
   only) into `plugins/<id>/`. Do **not** copy `data.json` or third-party plugins.
2. Add `LICENSE` (match Bryan's usual license), `.gitignore`, `package.json` (tooling only), and
   `scripts/validate-manifests.mjs` (manifest parses; required fields present; `id` == folder name; version is valid
   `x.y.z`; `main.js` parses under Node). Wire `npm run validate`.
3. Write a **great `README.md`**: what the repo is, the six plugins table (name, id, version, one-line description),
   repo layout, how plugins are developed (plain JS, manifest shape Obsidian loads), how they are deployed to the vault
   via `bob plugins sync`, per-plugin independent versioning, and a note that this is a private personal monorepo (not
   for direct official publishing).
4. Run `npm run validate`; confirm all six manifests pass. Commit & push.

**Done when:** repo holds the six validated plugin folders + README + LICENSE + validation tooling, pushed to origin.
**Handoff:** the `plugins/<id>/` layout is the input contract for Phases 2–3.

### Phase 2 — `bob plugins` scaffold + `list` (default) in bob-cli

_Repo: this repo. Depends on: Phase 1 (repo layout to read/test against)._

1. Register the command: add `NativeCommand::Plugins` to `src/native.rs`, a `Subcommand`/clap delegate in
   `src/runner.rs` `SUBCOMMANDS` in **alphabetical position** (`plugins` sorts before `pomodoro`), and an `AFTER_HELP`
   example.
2. Add `src/native/plugins.rs` modeled on `projects.rs`: clap builder with `list`/`sync` subcommands declared (sync may
   be a stub returning a clear "implemented in next phase"/`todo` is **not** acceptable for a shipped phase — if Phase 3
   is separate, register `sync` here but keep it minimal; preferred: leave `sync` wiring to Phase 3 and only ship
   `list` + shared scaffolding). Implement repo/vault discovery, manifest parsing, `community-plugins.json` reading, and
   the shared per-plugin model used by both subcommands.
3. Implement `run_list` per the design above: beautiful aligned colored table, header + footer summary, `-r/--repo`,
   `-b/--bob-dir`, `-f/--format`. No subcommand → `list`.
4. Update surfaces: `justfile` `install-smoke` (`bob plugins --help`, `bob plugins list --help`), `README.md` (new
   section), add `docs/plugins.md`. Add integration tests to `tests/cli.rs` (help text, sorted help, `list` table
   - `--format json`, default-subcommand behavior). Keep the `subcommands_are_sorted_alphabetically` test green.
5. `just fix` then `just all` (fmt + clippy + test) clean.

**Done when:** `bob plugins` and `bob plugins list` work and are beautiful; tests/docs/help updated; `just all` green.
**Handoff:** shared scaffolding (discovery + plugin model) is reused by Phase 3.

### Phase 3 — `bob plugins sync` in bob-cli

_Repo: this repo. Depends on: Phase 2 (scaffold)._

1. Implement `run_sync`: repo → vault copy of `manifest.json`/`main.js`/ `styles.css` only; `-p/--plugin`,
   `-d/--dry-run`, `-F/--force`; refuse to overwrite vault files that are dirty in the vault git repo unless `--force`;
   colored per-plugin action report.
2. Register `sync` in the `plugins` clap builder (sorted), add `AFTER_HELP` examples, extend `install-smoke`
   (`bob plugins sync --help`), document in `docs/plugins.md` + README, and add `tests/cli.rs` coverage (dry-run,
   single-plugin, dirty-file refusal, data.json preservation) using a temp fixture repo + vault.
3. `just fix` then `just all` clean.

**Done when:** `bob plugins sync` deploys safely (verified on the smallest plugin `bob-project-tasks` first), with
dry-run + force + dirty-file guards; tests/docs green.

### Phase 4 — Adopt `bob-plugins` as a SASE sibling + `sase init` + verify

_Repo: this repo. Depends on: Phase 1 (repo must exist); best run after 2–3 to verify end to end._

1. Add a `sibling_repos:` block to `sase.yml` (model on `~/projects/github/sase-org/sase/sase.yml`) with one entry:
   `name: bob-plugins`, `path: ~/projects/github/bbugyi200/bob-plugins` (absolute `~`-path — bob-cli's workspace layout
   is not adjacent to its canonical repo, so relative `../bob-plugins` is unsafe), and a description (e.g.
   "Source-of-truth monorepo for Bryan's custom Bob Obsidian plugins, deployed to the vault via `bob plugins sync`.").
2. Run `sase init` (use `-y` if non-interactive). Confirm no "primary path does not exist" sibling warning and that
   initialization succeeds.
3. End-to-end smoke: `bob plugins list` shows all six as `synced` + `enabled`; `bob plugins sync --dry-run` reports no
   changes against a freshly-synced vault. Commit.

**Done when:** `sase.yml` declares the `bob-plugins` sibling, `sase init` ran cleanly, and the full `bob plugins`
workflow is verified.

## Verification (cross-cutting)

- `just all` (fmt + clippy + test) green for every bob-cli phase.
- `bob plugins --help`, `bob plugins list --help`, `bob plugins sync --help` are excellent and alphabetized; options
  have short aliases.
- `npm run validate` green in bob-plugins.
- No third-party plugins, `data.json`, or vault notes leak into bob-plugins.

## Open questions (surface to user; safe defaults chosen)

1. **Vault as second source of truth.** Default: keep the six folders tracked in the vault and use `bob plugins sync` to
   deploy. Do you want a follow-up to `git rm --cached` them from `~/bob` so the repo is the _sole_ source (affects
   other machines)?
2. **History.** Default: snapshot import. Want `git filter-repo`/`subtree split` to preserve plugin history instead?
3. **`BOB_PLUGINS_DIR` default.** OK to hardcode the default repo path as `~/projects/github/bbugyi200/bob-plugins`
   (overridable via env / `--repo`)?
4. **Public split.** Any plugin (e.g. `bob-vim-surround`, `block-id-prompt`) slated for BRAT/official publishing soon,
   which would argue for its own repo?
