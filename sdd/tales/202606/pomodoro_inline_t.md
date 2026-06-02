---
create_time: 2026-06-02 11:41:53
status: done
prompt: sdd/prompts/202606/pomodoro_inline_t.md
---
# Pomodoro Inline `t` Duration Plan

## Goal

Move Bob's Pomodoro duration tracking from generated custom text to Dataview task inline fields:

- remove the now-unused `bob pomodoro-runtimes` command and its compatibility surfaces;
- have the Obsidian `se...<Tab>` ledger snippet create `[t:: ...]` duration metadata inside the time-range parentheses;
- update the daily-note Pomodoros heading template to render an inline Dataview sum of completed task durations;
- update Obsidian Vim-mode `\p` and `\P` so they adjust `[t:: ...]` instead of changing the time range end.

The intended new ledger shape is:

```markdown
## Pomodoros (`= durationformat(default(sum(nonnull(map(filter(this.file.tasks, (task) => task.completed AND startswith(meta(task.section).subpath, "Pomodoros")), (task) => task.t))), dur("0m")), "h'h' m'm'")`)

- [x] (0900-0925 [t:: 25m]) #task Example completed Pomodoro
- [ ] (0930-0955 [t:: 25m]) #task Next Pomodoro
```

## Context Reviewed

- Obsidian vault context was read through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault/template/snippet/keymap context before planning Pomodoro duration inline-field migration"`.
- `~/bob` is the Obsidian vault and is actively synced; its `AGENTS.md` requires checking status before edits and
  committing only task-related vault changes after implementation.
- `bob-cli` currently owns `bob pomodoro`, `bob tmux-pomodoro`, and `bob pomodoro-runtimes`.
- `bob pomodoro-runtimes` is wired through:
  - `Cargo.toml`
  - `src/bin/bob_pomodoro_runtimes.rs`
  - `src/native.rs`
  - `src/native/runtimes.rs`
  - `src/runner.rs`
  - `src/scripts.rs`
  - `scripts/bob_pomodoro_runtimes`
  - README, justfile, fixtures, and integration tests.
- The live Obsidian snippet and Vim-mode mappings are in: `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`.
- The daily template is: `/home/bryan/bob/_templates/daily.md`.
- There is still a chezmoi-managed `bob_pomodoro_runtimes` wrapper at:
  `/home/bryan/.local/share/chezmoi/home/bin/executable_bob_pomodoro_runtimes`.

## Product Decisions

1. Store Pomodoro durations as Dataview duration values in `[t:: Nm]`.
   - Use minutes as the canonical stored format, e.g. `[t:: 25m]` and `[t:: 100m]`.
   - Dataview 0.5.68 parses inline field durations like `25m`, and `durationformat(...)` can format the summed result.

2. Keep the time range as actual start/end clock time.
   - `\o` and `\O` should continue moving the clock range earlier/later.
   - `\p` and `\P` should adjust only `[t:: ...]`.
   - This separates "when the Pomodoro happened" from "how much duration should count toward the section total."

3. Do not bulk-migrate historical notes in this change.
   - The template/snippet affect new notes and new ledger entries.
   - The Obsidian keymap should opportunistically migrate the current line if it sees legacy `⏱️ ...` duration text, but
     the implementation should not scan and rewrite the whole vault.

4. Keep `bob pomodoro` and `bob tmux-pomodoro` working with the new inline-field-in-parentheses format.
   - New open Pomodoro lines will contain `[t:: ...]`, so status parsing must accept metadata after the end time inside
     the range parentheses.

## Bob CLI Changes

Remove the runtime annotation command completely:

- Delete the `pomodoro-runtimes` subcommand from `src/runner.rs`, including the top-level help example.
- Remove `NativeCommand::PomodoroRuntimes`, `mod runtimes`, the script-command mapping for `bob_pomodoro_runtimes`, and
  the dispatch to `runtimes::run`.
- Delete `src/native/runtimes.rs`.
- Delete `src/bin/bob_pomodoro_runtimes.rs` and remove its `[[bin]]` entry from `Cargo.toml`.
- Delete `scripts/bob_pomodoro_runtimes` and remove its embedded asset from `src/scripts.rs`.
- Update `justfile`:
  - remove the Python compile check for the deleted script;
  - remove `bob pomodoro-runtimes --help` from install smoke checks.
- Update `README.md`:
  - remove the command documentation;
  - remove `bob_pomodoro_runtimes` from compatibility shims;
  - remove runtime dependency/environment text that exists only for runtime annotation;
  - update smoke-test and release-check examples;
  - update fallback wording from Bash/Python to the remaining script reality.
- Remove runtime-command fixtures and tests from `tests/cli.rs` and `tests/fixtures/pomodoro_runtimes/`.
- Update cache-extraction tests to force script extraction through a remaining script-backed command, such as
  `bob notify --help`, and expect only the remaining embedded script assets.
- Update top-level help assertions to remove `pomodoro-runtimes`.

Preserve and update Pomodoro status parsing:

- Update native `src/native/pomodoro.rs` so `task_time_range` accepts both compact and colon ranges with trailing
  metadata before the closing parenthesis, such as `(0945-1015 [t:: 30m])`.
- Update `clean_task` so it removes the whole time-range parenthetical, including `[t:: ...]`, before removing other
  inline fields from the displayed task text.
- Update fallback `scripts/bob_pomodoro` Perl parsing the same way so `BOB_CLI_USE_SCRIPT=1` remains equivalent.
- Add or update integration coverage so `bob pomodoro` / `bob tmux-pomodoro` still output the expected status when the
  selected open ledger line contains `[t:: ...]` inside the range parentheses.

## Obsidian Plugin Changes

Update `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`:

1. Pomodoros heading detection:
   - Change the exact `## Pomodoros` heading matcher to accept trailing heading content.
   - This is required because the template heading will contain an inline Dataview query.

2. `se...<Tab>` snippet:
   - Extend `computeRange(...)` or `computeSnippetExpansion(...)` to include the computed duration minutes.
   - For ledger triggers, emit `(HHMM-HHMM [t:: Nm])`.
   - Preserve existing trigger semantics:
     - `se` -> 25 minutes;
     - `seN` -> `N * 5` minutes;
     - `seN-` -> `N * 5` minutes from `now - 5m`;
     - `seN-M` -> `N * 5` minutes from `now - M*5m`;
     - `se0` -> `0m`;
     - midnight wrapping still works.
   - Keep placeholder behavior clean:
     - `- [ ] (se) #task` becomes `- [ ] (HHMM-HHMM [t:: Nm]) #task`;
     - bare `se` still appends the trailing space after the inserted parenthetical.

3. Time-range parsing:
   - Allow metadata after the end time inside the parentheses.
   - Preserve `[t:: ...]` when `\o` and `\O` rewrite the range.
   - Avoid treating the metadata as part of the end time.

4. Duration-field helpers:
   - Add helper logic to find `[t:: ...]` in the range parentheses.
   - Parse at least the canonical `Nm` format; also accept common legacy display forms like `1h40m` / `1h 40m` when
     migrating old stopwatch text.
   - Format updated values canonically as `[t:: Nm]`.
   - If `[t:: ...]` is missing, seed it from the parsed start/end range duration.
   - If legacy `⏱️ ...` text is present inside the parentheses, replace it with `[t:: ...]` when `\p` or `\P` edits that
     line.
   - Clamp decrements at `0m` rather than writing negative durations.

5. Vim-mode mappings:
   - Keep the same normal-mode mappings and repeat-count behavior.
   - Change `bobLedgerAddPomodoroUnit` / `\p` to add `count * 5m` to `[t:: ...]`.
   - Change `bobLedgerSubtractPomodoroUnit` / `\P` to subtract `count * 5m` from `[t:: ...]`.
   - Leave `\o` / `\O` as range-offset operations.
   - Update helper export names or add exports as needed for Node-level verification.

## Daily Template Change

Update `/home/bryan/bob/_templates/daily.md`:

```markdown
## Pomodoros (`= durationformat(default(sum(nonnull(map(filter(this.file.tasks, (task) => task.completed AND startswith(meta(task.section).subpath, "Pomodoros")), (task) => task.t))), dur("0m")), "h'h' m'm'")`)
```

The `startswith(...)` guard is intentional because the heading itself now contains the inline query text. It should
still match the raw section subpath.

## Chezmoi Cleanup

Because `bob_pomodoro_runtimes` still exists as a chezmoi-managed wrapper:

- delete `/home/bryan/.local/share/chezmoi/home/bin/executable_bob_pomodoro_runtimes`;
- apply that removal to the live file if needed so `~/bin/bob_pomodoro_runtimes` does not keep calling a removed
  subcommand;
- keep the chezmoi change separate from Bob CLI and vault changes in status/commit handling.

Do not delete the stale Cargo-installed binary directly as part of implementation unless explicitly requested. The final
answer should mention that an already-installed `~/.cargo/bin/bob_pomodoro_runtimes` may remain until reinstall cleanup.

## Verification

Bob CLI:

```bash
cargo fmt --check
cargo test
just check-scripts
cargo package --list
```

Run a local install smoke test adjusted to the new command list:

```bash
root="$(mktemp -d)"
cargo install --path . --locked --root "$root"
"$root/bin/bob" collect-done --help >/dev/null
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" pomodoro >/dev/null
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" tmux-pomodoro >/dev/null
"$root/bin/bob_notify" --help >/dev/null
```

Obsidian vault/plugin:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
jq '.' /home/bryan/bob/.obsidian/plugins/dataview/data.json
```

Run focused Node helper assertions with stubbed `obsidian`, `@codemirror/state`, and `@codemirror/view` modules for:

- `se<Tab>` expansion includes `[t:: 25m]`;
- `se1`, `se5-`, `se5-2`, `se0`, and midnight wrapping preserve existing timing semantics;
- placeholder expansion replaces `(se)` with one clean parenthetical;
- Pomodoros heading detection accepts the inline-query heading;
- time-range parsing accepts `(0800-0825 [t:: 25m])`;
- `\p`-equivalent helper increments `[t:: ...]` without changing the range;
- `\P`-equivalent helper decrements and clamps at zero;
- missing `[t:: ...]` is seeded from range duration;
- legacy `⏱️ ...` text is migrated on edit;
- `\o` / `\O` preserve `[t:: ...]` while moving the range.

Manual Obsidian checks after plugin reload:

- In a daily note, `se<Tab>` expands to `(HHMM-HHMM [t:: Nm])`.
- In Vim normal mode, `\p` and `\P` adjust the `t` value on the target Pomodoro line.
- `\o` and `\O` still move the range and keep the `t` value.
- The Pomodoros heading renders a summed duration for completed Pomodoro tasks.

Status/diff checks:

```bash
git status --short
git -C /home/bryan/bob status --short
git -C /home/bryan/.local/share/chezmoi status --short
```

## Risks

- The inline Dataview query is long and will make the raw heading noisy, but it keeps the summary self-maintaining and
  avoids another CLI rewrite pass.
- Existing historical `⏱️ ...` entries will not contribute to the new Dataview total until migrated manually or edited
  by the keymap; this plan intentionally avoids a bulk vault rewrite.
- `bob pomodoro` must be updated before the new snippet is used heavily, otherwise active entries with `[t:: ...]`
  inside parentheses will not parse correctly.
- Obsidian Sync can introduce unrelated vault changes. Implementation must check status before edits and avoid staging
  or committing unrelated note changes.
