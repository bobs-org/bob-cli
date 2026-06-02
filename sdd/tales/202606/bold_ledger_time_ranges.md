---
create_time: 2026-06-02 12:12:46
status: wip
prompt: sdd/prompts/202606/bold_ledger_time_ranges.md
---
# Bold Ledger Time Ranges Plan

## Goal

Make Bob Pomodoro ledger clock ranges visually stand out in Obsidian by formatting the time-range atom with Markdown
bold while preserving the current inline duration field:

```markdown
- [x] (**0900-0925** [t:: 25m]) #task Completed Pomodoro
- [ ] (**0930-0955** [t:: 25m]) #task Current Pomodoro
```

The intended bolding applies only to the clock range (`0900-0925` or `09:00-09:25`), not the enclosing parentheses, the
`[t:: ...]` duration field, or the task text. `bob pomodoro`, `bob tmux-pomodoro`, and compatibility shims should keep
printing normalized plain status text such as `0930-0955 Current Pomodoro`; Markdown `**` should never leak into CLI or
tmux output.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through the required audited command:
  `sase memory read long/obsidian.md --reason "Need Obsidian ledger/plugin context before planning bold Pomodoro time-range formatting"`.
- The main `bob-cli` worktree is clean.
- The Obsidian vault has unrelated pre-existing dirty notes, but the likely target files are currently clean:
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`, `/home/bryan/bob/_templates/schedule.md`, and
  `/home/bryan/bob/_templates/daily.md`.
- The vault's `AGENTS.md` requires checking status before edits, preserving unrelated changes, and committing only
  task-related vault file edits before finishing.
- Current Bob Pomodoro parsing lives in:
  - `src/native/pomodoro.rs` for native `bob pomodoro`, native `bob tmux-pomodoro`, and native compatibility shims.
  - `scripts/bob_pomodoro` for `BOB_CLI_USE_SCRIPT=1` fallback behavior.
- Current CLI coverage uses `tests/fixtures/pomodoro/day_with_open_pomodoro.md` through native `bob tmux-pomodoro` and
  script-backed `bob pomodoro`.
- Current Obsidian range generation and editing lives in `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- `/home/bryan/bob/_templates/schedule.md` also emits Pomodoro range atoms and should be brought forward to the same
  ledger shape.

## Product Decisions

1. Canonical new ledger syntax is `(**HHMM-HHMM** [t:: Nm])`.
   - Compact ranges remain the default for generated snippets.
   - Existing colon-style ranges should remain supported and, when rewritten, become `(**HH:MM-HH:MM** [t:: Nm])`.

2. Parsing remains backward compatible.
   - Bob should continue accepting historical unbolded ranges like `(0945-1015)` and `(0945-1015 [t:: 30m])`.
   - Bob should also accept bolded ranges with metadata like `(**0945-1015** [t:: 30m])`.
   - The Obsidian plugin should find and edit both plain and bold ranges.

3. Rewrite operations should canonicalize to bold.
   - `se...<Tab>` should emit bold ranges immediately.
   - Vim actions that rebuild the ledger atom (`\p`, `\P`, `\o`, `\O`) should output bold ranges while preserving or
     normalizing `[t:: ...]` metadata as they already do.
   - Editing an old plain range with these commands can opportunistically migrate that line to the bold format.

4. Do not bulk-migrate historical vault notes in this change.
   - This keeps the change focused and avoids touching unrelated or dirty note content.
   - Existing notes remain readable by both Bob and the plugin.

5. The `bob` subcommand list does not need to change.
   - The required Bob updates are parser/test/doc compatibility for `pomodoro`, `tmux-pomodoro`, `bob_pomodoro`, and
     `tmux_bob_pomodoro`, not adding or removing command surfaces.

## Bob CLI Changes

Update native Pomodoro parsing in `src/native/pomodoro.rs`:

- Teach `task_time_range` to recognize an optional Markdown bold wrapper around the clock range inside the
  parenthetical.
- Keep extracting normalized `start` and `end` values without `**`.
- Keep `raw_range` as the full parenthetical so `clean_task` removes `(**HHMM-HHMM** [t:: Nm])` entirely before
  stripping other field links.
- Preserve support for:
  - compact ranges: `(0945-1015)`;
  - compact ranges with metadata: `(0945-1015 [t:: 30m])`;
  - bold compact ranges with metadata: `(**0945-1015** [t:: 30m])`;
  - colon variants where currently supported.

Update script fallback parsing in `scripts/bob_pomodoro`:

- Mirror the native parser behavior in the embedded Perl range matcher.
- Ensure `BOB_CLI_USE_SCRIPT=1 bob pomodoro` also strips `**` and returns the same normalized status output as native
  execution.

Update tests and fixtures:

- Change `tests/fixtures/pomodoro/day_with_open_pomodoro.md` so the selected open Pomodoro uses the new bold range with
  `[t:: ...]`.
- Add or adjust integration coverage so native `bob pomodoro`, native `bob tmux-pomodoro`, and script-backed
  `bob pomodoro` all parse the bold fixture and print plain `0945-1015 Review crate skeleton`.
- Keep at least one old-format range in the fixture or add a small focused fixture so backward compatibility for
  unbolded ranges remains covered.

Update documentation where useful:

- If README text or examples describe Pomodoro ledger syntax, update them to show the bold range syntax and state that
  CLI output remains plain.
- Leave command-list/help assertions unchanged except where fixture output expectations require updates.

## Obsidian Plugin And Template Changes

Update `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`:

- Extend `parseTimeRange` to accept optional paired Markdown `**` around the range for both compact and colon formats.
- Track enough parsed state to rebuild a range canonically with bold while preserving:
  - compact vs colon style;
  - start/end times;
  - trailing metadata such as `[t:: 25m]`;
  - legacy duration cleanup behavior already present for `\p` and `\P`.
- Update `formatTimeRange`, `replaceTimeRange`, `changePomodoroLineUnits`, `offsetPomodoroLineRange`, and `computeRange`
  so generated or rewritten ranges use `(**start-end** metadata)`.
- Keep helper exports available for focused Node verification.

Update `/home/bryan/bob/_templates/schedule.md`:

- Update comments and emitted lines from `(HHMM-HHMM)` to `(**HHMM-HHMM** [t:: 25m])` so this template matches the
  current daily Pomodoro ledger format.

No change is expected for `/home/bryan/bob/_templates/daily.md` unless implementation inspection finds an example there
that should mention the new range shape; the Pomodoros heading Dataview query can remain as-is.

## Verification Plan

Run Bob CLI checks:

```bash
cargo fmt
cargo fmt --check
cargo test pomodoro
cargo test
just check-scripts
cargo package --list --allow-dirty
just install-smoke
```

Run Obsidian/plugin checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
```

Run focused Node helper assertions with stubbed `obsidian`, `@codemirror/state`, and `@codemirror/view` modules to
cover:

- `se<Tab>` expansion returns `(**HHMM-HHMM** [t:: Nm])`;
- `parseTimeRange` accepts old plain ranges and new bold ranges;
- `formatTimeRange` / `replaceTimeRange` produce canonical bold ranges while preserving metadata;
- `changePomodoroLineUnits` updates `[t:: ...]` and keeps or migrates the range to bold;
- `offsetPomodoroLineRange` moves start/end, preserves `[t:: ...]`, and keeps or migrates the range to bold;
- colon-style bold ranges continue to parse and rewrite correctly.

Before finishing after implementation:

- Re-check `git status` in the main repo and vault.
- If vault files were changed, commit only the task-related vault files using the required `/sase_git_commit` workflow,
  leaving the pre-existing dirty notes untouched.
- Report any skipped validation, remaining dirty files, and whether the Bob CLI repo changes were committed or left
  uncommitted according to the finalizer/user instructions.

## Risks

- Markdown bold markers inside the range can confuse simple range parsers. Mitigation: make both native and fallback
  parsers explicitly strip/ignore the optional paired `**` before normalizing times.
- Broad regexes could accidentally parse non-ledger parentheses. Mitigation: keep the parser anchored to recognized time
  tokens and continue scanning only inside the Pomodoros section for Bob status.
- The vault has unrelated dirty notes. Mitigation: touch only clean plugin/template files and stage/commit only those
  task files if implementation proceeds.
