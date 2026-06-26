---
create_time: 2026-06-26 08:29:43
status: wip
prompt: sdd/prompts/202606/plugins_sync_safety.md
---
# Plan: Make `bob plugins sync` safe — diffs, dry-run previews, and backups

> **Target repository:** `bob-cli`. All paths are relative to the `bob-cli` repo root. The user-facing surface is the
> `bob plugins sync` subcommand, implemented in `src/native/plugins.rs`.

## Problem & motivation

`bob plugins sync` overwrites files **inside the Obsidian vault** (`<bob-dir>/.obsidian/plugins/<id>/`) from the
`bob-plugins` repo. Today it does this fairly bluntly:

- It byte-compares each managed file (`manifest.json`, `main.js`, `styles.css`) against the vault copy, but it **never
  shows what is changing**. The output says `copied main.js` — not _how_ it changed. You overwrite blind.
- It **keeps no copy of what it destroys.** A clean-in-Git vault file is recoverable via the vault's Git history, but a
  forced (`-F`) overwrite of a dirty file is gone forever, and even for clean files "go dig it out of git" is not a
  safety net you want to lean on for a hand-edited plugin.
- `-d/--dry-run` already exists, but it only prints `would copy main.js` — the same blind preview, just without writing.
  It does not let you _review_ the change before you commit to it.

The one real guard today — refusing to clobber a vault file that is dirty in Git unless `-F` is given — is good, but it
is the only safety mechanism, and it is binary (skip or destroy). This plan adds the three the user asked for, so a sync
is something you can **inspect before and recover after**:

1. **Show a diff** between the current vault file and the incoming repo file for every file that would change.
2. **`-d/--dry-run`** shows those same diffs (it already skips writes; now it becomes a real review tool).
3. **Back up every overwritten vault file** to a timestamped location before clobbering it, and print exactly where each
   backup lives.

The bar set by the user is explicit: **intuitive, reliable, and beautiful.** This plan treats those as acceptance
criteria, not adjectives.

## Goals

- Every file `sync` would overwrite is shown as a colored unified **diff** (old = current vault file, new = incoming
  repo file), in both a real sync and a dry-run.
- `-d/--dry-run` renders the identical diffs and the identical "where the backup would go" lines, but touches nothing on
  disk — a true preview of the real run.
- Every **overwrite** (a clean update _or_ a forced overwrite of a dirty file) first copies the file being destroyed to
  a **backup**, and the output states the backup path plainly. A failed backup **aborts that file's overwrite** — we
  never destroy a file we could not first preserve.
- Backups from one invocation are grouped under a single **timestamped run directory** so a sync's casualties are found
  in one place, and a later sync can never overwrite an earlier sync's backups.
- The output stays **beautiful and scannable**: a one-line-per-file summary you can skim, the diff body indented beneath
  it, large/minified/binary diffs gracefully summarized instead of vomiting 10k lines, and full color that degrades to
  clean ASCII when piped (so existing `assert_stdout_has_no_ansi` tests and real pipelines stay correct).
- The whole thing is **deterministically testable** — diffs and backup paths are exercisable in `tests/cli.rs` without a
  TTY and without wall-clock flakiness.

## Non-goals

- **No change to what gets synced.** The managed-file set (`manifest.json`, `main.js`, `styles.css`) and the absolute
  refusal to touch `data.json` / runtime files are unchanged.
- **No change to the dirty-file guard's policy.** A dirty vault file is still skipped without `-F` and overwritten with
  `-F`. We only _show the diff_ of what is being skipped/forced and _back up_ the forced casualty.
- **No new JSON surface.** `sync` is human-output only today (unlike `list`); it stays that way. Diffs and backups are a
  human-review feature.
- **No automatic backup pruning / GC.** Backups accumulate under the run directory. A retention/cleanup command is a
  reasonable future follow-up, explicitly out of scope here. (The footer prints the root so it is easy to find and clear
  manually.)
- **No interactive confirmation prompt.** The safety model is "preview with `-d`, then run" plus "backups on every
  overwrite" — not a per-file y/n. (Sync is often run non-interactively after `bob plugins sync`; a blocking prompt
  would fight that. Mentioned as a possible future flag, not built.)

## Background: how sync works today

`src/native/plugins.rs` (clap 4.5, builder style):

- `run_sync` (≈L246) builds `SyncOptions { repo, bob_dir, only, dry_run, force }` and calls `sync_plugins`, then
  `print_sync_report`, then prints `issues` to stderr and returns `0`/`1`.
- `sync_one_file` (≈L450) is the choke point and **already has both byte buffers in hand**:
  - reads `repo_bytes`; if the vault file exists, reads `vault_bytes`;
  - equal → `Unchanged`;
  - differ → dirty-check; dirty and not `--force` → `SkippedDirty`; else write (unless `dry_run`) and return `Created` /
    `Updated` / `Forced`.
- `FileAction` enum: `Created`, `Updated`, `Forced`, `Unchanged`, `SkippedDirty`, `Failed`. `is_copy()` = the three
  write actions; `is_warning()` = `SkippedDirty | Failed`.
- `print_sync_report` (≈L513) renders the header, one `<prefix> <id>  <detail>` line per changed file (or `up to date`),
  and a `N copied · N skipped · N unchanged` footer. Prefixes/verbs already branch on `dry_run` ("would copy" / "to
  copy").
- `Styler` (`src/native/style.rs`) gives `green`/`yellow`/`red`/`cyan`/`dim`/`blue`/`paint`, a `separator()` (`·` in
  color, `-` plain), and is **color iff stdout is a TTY and `NO_COLOR` is unset** — so piped output is already plain.
- `env.rs` already depends on `chrono` and exposes `current_datetime() -> NaiveDateTime`, which honors a **`BOB_NOW`**
  override. This is the testable clock for backup directory names — no new time plumbing required.

**Key insight:** the exact place I need to compute a diff and take a backup — `sync_one_file` — already holds the old
bytes (`vault_bytes`) and the new bytes (`repo_bytes`) and already knows the action. The feature is mostly _surfacing
information the function already computes_ plus a copy-before-write, not new traversal logic.

## Design

### Decision 1 — Diff direction and which files get one

A diff is always **old = current vault file → new = incoming repo file** (what you have → what sync would install). Per
action:

| Action         | Diff shown?                             | Backup?                  |
| -------------- | --------------------------------------- | ------------------------ |
| `Updated`      | yes (vault → repo)                      | yes (before overwrite)   |
| `Forced`       | yes (dirty vault → repo)                | yes (before overwrite)   |
| `SkippedDirty` | yes (so you see what you're _keeping_)  | no (nothing overwritten) |
| `Created`      | no body — compact `(new file, N lines)` | no (nothing to destroy)  |
| `Unchanged`    | no                                      | no                       |

Rationale: the safety concern is **overwriting** — those get a full diff + a backup. `SkippedDirty` still gets a diff
because seeing the divergence is exactly how you decide whether to `-F`. `Created` writes a brand-new file (nothing is
at risk), so a full body would be noise; a one-line "new file" note is enough. This mapping is the heart of "intuitive"
— the loud output is reserved for the genuinely destructive cases.

### Decision 2 — Diffs and backups are computed in `sync_one_file`, carried in the report, rendered separately

Keep the clean separation the module already has (compute in `sync_*`, render in `print_sync_report`). Extend the
per-file result so the renderer has everything it needs and the logic stays unit-testable without capturing stdout:

```rust
struct FileSync {
    name: String,
    action: FileAction,
    diff: Option<FileDiff>,          // Some for Updated/Forced/SkippedDirty/Created; None for Unchanged
    backup: Option<BackupOutcome>,   // Some only when an overwrite happened or (in dry-run) would happen
}

enum FileDiff {
    Text { lines: Vec<DiffLine>, added: usize, removed: usize, hidden: usize }, // hidden = truncated-away lines
    Binary { old_len: usize, new_len: usize },                                  // non-UTF-8 content
    NewFile { lines: usize, bytes: usize },                                     // Created
}

struct DiffLine { kind: DiffKind, text: String }
enum DiffKind { Hunk, Context, Add, Del }   // Hunk = the "@@ -a,b +c,d @@" header

struct BackupOutcome { path: PathBuf, written: bool }  // written == false in dry-run
```

All of these derive `Debug, Clone, PartialEq, Eq` so the existing `assert_eq!(action_for(...), ...)`-style unit tests
keep working and new ones can assert on diff/backup contents directly.

### Decision 3 — Backups: location, layout, and fail-safe ordering

**Location (outside the vault, on purpose).** Backups must not land inside `<bob-dir>`: the vault is a Git repo, and
writing backup files into it would pollute `git status`, risk being accidentally committed, and muddy the very
dirty-check this command relies on. Resolution order for the backup base directory:

1. `-B, --backup-dir DIR` (new flag, see Decision 5),
2. else `BOB_PLUGIN_BACKUPS_DIR` env var,
3. else default `~/.local/state/bob-cli/plugin-backups` (XDG-state-style; via a new `bob_env::plugin_backups_dir()` that
   mirrors the existing `bob_dir()` / `plugins_dir()` helpers).

**Layout — one timestamped directory per invocation:**

```
<backup-base>/<timestamp>/<plugin-id>/<filename>
e.g.  ~/.local/state/bob-cli/plugin-backups/20260626-143000/bob-project-tasks/main.js
```

The `<timestamp>` is computed **once** at the start of `run_sync` from `bob_env::current_datetime()` formatted
`%Y%m%d-%H%M%S`, and stored on `SyncOptions` as a fully-resolved `backup_run_dir`. One run → one directory; every file
within a run is uniquely keyed by `<plugin-id>/<filename>`, so no intra-run collisions, and a later run gets a later
timestamp so it can never clobber an earlier run's backups. (Using `current_datetime()` means a test can pin `BOB_NOW`
and assert the exact backup path.)

**Fail-safe ordering in `sync_one_file`** — backup strictly precedes the destructive write:

1. Determine the action would be `Updated` or `Forced` (i.e. an existing file will be overwritten).
2. If not `dry_run`: create the backup's parent dir, then `fs::copy(vault_file → backup_path)`. **If the copy fails,
   push an issue, return `FileAction::Failed`, and do _not_ write** — we never destroy what we could not first save.
3. Only then `fs::write(vault_file, repo_bytes)`.
4. Record `BackupOutcome { path, written: !dry_run }`.

In `dry_run`, steps 2–3 are skipped but the `backup_path` is still computed and recorded (`written: false`) so the
preview can honestly say _where_ the backup would go.

### Decision 4 — The diff engine: add the `similar` crate

Producing a correct, beautiful unified diff (Myers, with hunk grouping and `@@` headers) is not something to hand-roll —
a naive LCS is `O(n·m)` memory and falls over on a large bundled `main.js`. I propose adding
[`similar`](https://crates.io/crates/similar) (the diff library behind `insta`/`ruff`: pure Rust, well-maintained, no
heavy transitive deps). The project already curates quality crates (`chrono`, `regex`, `lopdf`, `sha2`), so one more
focused crate is in keeping. Usage: `TextDiff::from_lines(old, new).unified_diff().context_radius(3)`, iterate hunks →
`DiffLine`s; count `Insert`/`Delete` for the `+N -M` stat.

> **Alternative considered (flagged for your call):** a dependency-free hand-rolled line diff. Rejected as the default
> because diff quality _is_ the feature here and "reliable + beautiful" is the explicit bar; a bespoke differ is more
> code, more risk, and worse on pathological inputs. If you'd rather not take the dependency, say so and I'll fall back
> to a bounded hand-rolled differ with the same rendering.

**Robustness rules so the diff never becomes an eyesore:**

- **Binary / non-UTF-8** content (either side not valid UTF-8) → `FileDiff::Binary`, rendered as
  `binary file differs (1.2 KB → 1.4 KB)`. No garbage bytes to the terminal.
- **Per-line width cap.** Each rendered diff line is truncated to the terminal width with a trailing `…` (reusing the
  existing `truncate` / `display_width` / `terminal_width` helpers), so a minified one-line `main.js` can't blow out the
  screen horizontally.
- **Per-file line cap.** Cap the diff body at a constant (≈60 changed+context lines). Excess is collapsed into a dim
  `… and N more changed lines (full file backed up at <path>)` — pointing at the backup, which is the complete record.
- **Minified heuristic.** If a side has very few newlines but is large (e.g. a bundled build), skip the line diff
  entirely and show a `FileDiff::Binary`-style byte-size summary — a hunk diff of one 50 KB line helps no one.

### Decision 5 — One new option: `-B, --backup-dir`; everything else already exists

`-d/--dry-run` and `-F/--force` already exist, so the only new flag is the backup-directory override. Final `sync`
option set (kept **alphabetical by long name** per the CLI rules, every long option keeps a short alias):

```
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run]
                 [-F|--force] [-p|--plugin ID] [-r|--repo DIR]
```

`-B, --backup-dir DIR` — help:
`Directory for backups of overwritten vault files; defaults to BOB_PLUGIN_BACKUPS_DIR or ~/.local/state/bob-cli/plugin-backups`.
(`-B` pairs naturally with the existing `-b` for `--bob-dir`.) The `--help` output gets the same care as today — the new
flag slots in first and the `assert_text_order` test is updated to match.

> **Deferred (your call):** a `-q/--quiet` that suppresses diff _bodies_ (keeping the one-line summaries + backup paths)
> for the "I already reviewed with `-d`, just sync it" case. Not in core scope because the user explicitly asked to
> _show_ diffs; the per-file/per-line caps already keep the default readable. Easy to add later if the default proves
> noisy.

### Decision 6 — Output design (intuitive + beautiful)

Keep the proven scannable skeleton (header → per-file status line → footer) and nest the new detail _under_ each changed
file, so a skim still reads top-to-bottom in one line per file, and the diff/backup detail is there when you look.

Real run:

```
Bob Plugins · sync · ~/projects/github/bobs-org/bob-plugins -> ~/bob

  ok       bob-project-tasks  copied main.js   +12 -3
             @@ -1,5 +1,6 @@
              export class ProjectTasks {
            -   const limit = 1;
            +   const limit = 2;
            +   const verbose = true;
             }
             ↳ backed up to ~/.local/state/bob-cli/plugin-backups/20260626-143000/bob-project-tasks/main.js
  ok       block-id-prompt    copied manifest.json (new file, 5 lines)
  warning  daily-note-helper  skipped main.js (dirty in vault; use -F/--force)   +2 -2
             @@ -3,3 +3,3 @@
            -   const delay = 500;
            +   const delay = 250;

  2 copied · 1 skipped · 4 unchanged · backups in ~/.local/state/bob-cli/plugin-backups/20260626-143000
```

Dry-run (`-d`) — identical diffs, nothing written, verbs and the backup line switch to conditional:

```
  [dry-run] ok  bob-project-tasks  would copy main.js   +12 -3
             @@ -1,5 +1,6 @@
              ...same diff...
             ↳ would back up to ~/.local/state/bob-cli/plugin-backups/20260626-143000/bob-project-tasks/main.js

  2 to copy · 1 skipped · 4 unchanged · backups would go in ~/.local/state/bob-cli/plugin-backups/20260626-143000
```

Styling: `@@` hunk headers `dim`, `+` adds `green`, `-` deletes `red`, context default, the `↳ backed up` line
`blue`/dim. **All diff/backup rendering goes through `Styler`,** so on a pipe it is plain ASCII (markers
`+`/`-`/`@@`/`↳` preserved for grep-ability and for tests). The `backups in …` footer fragment only appears when at
least one backup was/would be made.

## Implementation outline (by file)

1. **`Cargo.toml`** — add `similar = "2"` (latest 2.x) to `[dependencies]`; `cargo build` refreshes `Cargo.lock`.

2. **`src/native/env.rs`** — add `plugin_backups_dir() -> PathBuf` mirroring `plugins_dir()`: `BOB_PLUGIN_BACKUPS_DIR`
   (tilde-expanded) else `home_dir().join(".local/state/bob-cli/plugin-backups")`.

3. **`src/native/style.rs`** — small additive helpers if useful (e.g. a `diff_add`/`diff_del`/`hunk` convenience, or
   just reuse `green`/`red`/`dim`). Keep it minimal; no behavior change to existing methods.

4. **`src/native/plugins.rs`** (the bulk):
   - `sync_command()` / new `backup_dir_arg()`: add `-B, --backup-dir`, declared so help stays alphabetical.
   - `SyncOptions`: add `backup_run_dir: PathBuf` (already-resolved `<base>/<timestamp>`). Resolve in `run_sync` via a
     new `backup_dir_from_matches` + `bob_env::current_datetime().format("%Y%m%d-%H%M%S")`.
   - `FileSync` + new `FileDiff` / `DiffLine` / `DiffKind` / `BackupOutcome` types (Decision 2).
   - `sync_one_plugin`: compute the per-file backup target (`backup_run_dir/<id>/<name>`) and pass it to
     `sync_one_file`.
   - `sync_one_file`: build the `FileDiff` from the byte buffers it already has (UTF-8 / binary / minified branches per
     Decision 4); for `Created`, build `NewFile`; do the backup-before-write with fail-safe ordering (Decision 3);
     populate `diff` and `backup` on the returned `FileSync`.
   - A `diff_text(old: &str, new: &str) -> (Vec<DiffLine>, added, removed, hidden)` helper wrapping `similar`, with the
     line-count cap; plus the per-line width truncation applied at render time.
   - `print_sync_report`: after each changed file's status line (now with the `+N -M` stat), render the indented diff
     body and the `↳ backed up`/`would back up` line; add the `backups in …` footer fragment. Keep the existing
     status-line phrasing (`copied`/`would copy`/`skipped`/`up to date`/`to copy`) so the change is additive.
   - `run_sync`: print the backup-root summary; treat a backup failure (`FileAction::Failed`) as an issue → exit `1`.

5. **`docs/plugins.md`** — document the diffs, the backup location/layout/precedence, the `-B` flag, the dry-run preview
   behavior, and the failed-backup-aborts-overwrite rule. Update the `sync` usage line and the "For every managed file,
   sync reports one of" list. (Docs are part of "done" for this repo — `docs/plugins.md` is the canonical reference.)

6. **`memory/cli_rules.md`** — no change expected; the design already satisfies it (sorted alphabetical options, short
   aliases, excellent `--help`, colored output). Flagged only so the implementer re-checks against it. (Per AGENTS.md,
   memory files are not modified without your approval.)

## Behavior matrix (after the change)

| Situation                                | Action         | Diff shown            | Backup taken                    | Exit |
| ---------------------------------------- | -------------- | --------------------- | ------------------------------- | ---- |
| Vault file missing                       | `Created`      | `(new file, N lines)` | no                              | 0    |
| Vault file matches repo                  | `Unchanged`    | none                  | no                              | 0    |
| Clean vault file differs                 | `Updated`      | vault → repo          | yes, before overwrite           | 0    |
| Dirty vault file, no `-F`                | `SkippedDirty` | vault → repo          | no (untouched)                  | 0    |
| Dirty vault file, with `-F`              | `Forced`       | dirty → repo          | yes, before overwrite           | 0    |
| Any overwrite, but the backup copy fails | `Failed`       | (diff still computed) | attempted; **write aborted**    | 1    |
| `-d/--dry-run`, any would-overwrite      | as above       | same diff             | path shown, **nothing written** | 0    |
| Non-UTF-8 / minified file differs        | as above       | byte-size summary     | per the overwrite rule above    | 0/1  |

## Testing

Unit tests (in `src/native/plugins.rs`, alongside the existing `mod tests`):

- **Diff content** — `Updated`/`Forced`/`SkippedDirty` produce a `FileDiff::Text` with the expected `added`/`removed`
  counts and `@@` hunk line; `Created` → `NewFile`; `Unchanged` → `None`.
- **Backup taken before overwrite** — after an `Updated`/`Forced`, the backup path exists and its bytes equal the
  _original_ vault file; the vault file now equals the repo. With `BOB_NOW` pinned, assert the exact backup path.
- **Dry-run** — `backup.written == false`, no backup file on disk, vault unchanged (extends the existing
  `sync_dry_run_reports_without_writing`).
- **Skipped/Created never back up** — `SkippedDirty` and `Created` have `backup == None` and create no backup file.
- **Fail-safe** — when the backup copy can't be made (e.g. backup base under a read-only/again-a-file path), the action
  is `Failed`, an issue is recorded, and the vault file is left at its original contents (not overwritten).
- **Binary / minified** — non-UTF-8 and few-newlines-but-large inputs yield the byte-size summary, not a line diff.

Integration tests (`tests/cli.rs`, using the `write_plugins_fixture` helper, `BOB_NOW`, and a temp `-B` dir):

- **`plugins_sync_shows_diffs`** — a real sync's stdout contains the `@@` hunk and `+`/`-` lines for a drifted file, and
  `assert_stdout_has_no_ansi` still passes (color off when captured).
- **`plugins_sync_backs_up_overwritten_file`** — stdout contains `backed up to <path>`; that path exists with the old
  contents; footer contains `backups in`.
- **`plugins_sync_dry_run_*`** — extend the existing dry-run test: stdout shows the diff and `would back up to <path>`,
  but no file is written and the backup dir is not created.
- **`plugins_sync_help_lists_options_alphabetically`** — update to include `-B, --backup-dir` at the front of the
  `assert_text_order` list.
- Existing assertions (`would copy main.js`, `copied main.js`, `skipped main.js … dirty`, `to copy`, the
  `Bob Plugins - sync -` header) are preserved by keeping the status-line phrasing additive.

## Risks & mitigations

- **New dependency (`similar`).** Mitigation: it's a small, widely-used, pure-Rust crate; the diff quality is the
  feature. Hand-rolled fallback offered (Decision 4) if you'd rather not add it.
- **Diff noise on large/bundled `main.js`.** Mitigation: per-line width cap, per-file line cap with a "see backup"
  pointer, and the minified/binary summary path (Decision 4).
- **Backups polluting the vault Git repo.** Mitigation: backups default to a state dir _outside_ `<bob-dir>` and never
  inside the vault unless the user explicitly points `-B` there (Decision 3).
- **A destructive overwrite with no recoverable copy.** Mitigation: backup-before-write with the write aborted on backup
  failure — the central reliability guarantee (Decision 3).
- **Test flakiness from wall-clock timestamps.** Mitigation: backup directory names come from
  `bob_env::current_datetime()`, which honors `BOB_NOW`, so paths are deterministic in tests.
- **Output churn breaking existing tests.** Mitigation: the redesign is additive — status-line phrasing and the header
  are preserved; only the `assert_text_order` help list and the dry-run test gain new assertions.
- **Backup directory growth over time.** Acknowledged non-goal (no GC). Mitigation: the footer always prints the root so
  it is trivial to find and clear; a prune command is a clean future follow-up.
