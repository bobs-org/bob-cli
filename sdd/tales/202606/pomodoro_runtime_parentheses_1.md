---
create_time: 2026-06-02 10:09:11
status: done
prompt: sdd/prompts/202606/pomodoro_runtime_parentheses_1.md
---
# Pomodoro Runtime Parentheses Plan

## Context

`bob pomodoro-runtimes` currently annotates completed Pomodoro ledger task lines by appending a trailing runtime suffix:

```markdown
- [x] (09:00-09:25) Task text ⏱️ 25m
```

It also annotates the section heading with the total runtime:

```markdown
## Pomodoros ⏱️ 45m
```

The desired behavior is to keep the ledger/time-range parentheses where they already are, especially when they are at
the front of the task text, and put the per-task runtime inside those same parentheses:

```markdown
- [x] (09:00-09:25 ⏱️ 25m) Task text
```

The `## Pomodoros` heading should go back to the plain title so heading-sensitive tooling, including the `\\`
localleader keymap setup, is not broken by title annotations.

## Scope

- Update the native Rust implementation in `src/native/runtimes.rs`.
- Update the embedded Python fallback in `scripts/bob_pomodoro_runtimes` so `BOB_CLI_USE_SCRIPT=1` stays aligned.
- Update Bob CLI integration fixtures and tests in `tests/fixtures/pomodoro_runtimes/*.md` and `tests/cli.rs`.
- Update the Neovim Pomodoro keymap source and test in the chezmoi-managed config so the `\\` keymap still works when
  completed task lines contain runtime metadata inside the ledger parentheses.

## Design

1. Preserve the Pomodoros section boundary behavior.
   - Continue matching `## Pomodoros` even when legacy or current annotations are present.
   - Rewrite the heading back to the base `## Pomodoros` text on update.
   - Stop writing the total runtime to the heading.

2. Replace task-line suffix annotation with in-parentheses annotation.
   - For each completed ledger line in the Pomodoros section, strip any legacy trailing `[runtime:: ...]` suffix or
     trailing `⏱️ ...` suffix.
   - Locate the existing time-range parentheses in the line.
   - Parse the start and end times from the existing range without moving the range.
   - Strip any existing in-parentheses runtime annotation from that range.
   - Replace only the contents of the existing parentheses with `<original range> ⏱️ <duration>`.
   - Leave open task lines and lines outside the Pomodoros section untouched.

3. Preserve idempotence and migration behavior.
   - A second `bob pomodoro-runtimes` run should produce no changes.
   - Existing legacy heading totals should be removed.
   - Existing legacy trailing task runtimes should migrate into the ledger parentheses.
   - Existing in-parentheses task runtimes should be recalculated in place rather than duplicated.

4. Keep the `\\` keymap working.
   - Keep `## Pomodoros` plain after the CLI rewrite, which fixes the header-title regression.
   - Update the Neovim keymap time-range parser to accept optional trailing content inside the time-range parentheses,
     so lines like `(09:00-09:25 ⏱️ 25m)` are still recognized.
   - When a keymap edits a time range that already contains runtime metadata, it may replace the parenthesized range
     with a fresh plain time range; this avoids preserving stale runtime metadata after manual edits.

## Tests

- Update existing Bob CLI runtime tests to assert:
  - the heading is plain `## Pomodoros`;
  - completed tasks receive `⏱️ <duration>` inside the existing parentheses;
  - the ledger/time-range parentheses remain at the front when the fixture starts that way;
  - legacy `[runtime:: ...]` and trailing stopwatch suffixes are removed;
  - reruns are idempotent;
  - the script fallback behaves the same as native.
- Update/run the Neovim Pomodoro keymap spec to cover a completed line with an in-parentheses runtime.
- Run targeted checks first:
  - `cargo test pomodoro_runtimes`
  - `BOB_CLI_USE_SCRIPT=1` path covered by the existing integration test
  - Neovim keymap spec command from the config test suite
- If targeted checks pass, run the broader repo checks that are practical in this workspace:
  - `cargo fmt --check`
  - `cargo test`
  - `just check-scripts`

## Risks

- The Rust and Python implementations can drift if the parsing rules differ; keeping the fixtures shared through
  integration tests reduces that risk.
- Broadly accepting extra text inside ledger parentheses in the keymap parser could parse a non-runtime annotation, but
  the replacement behavior intentionally drops that extra text when manually editing the range to avoid stale runtime
  data.
- The Neovim keymap files live outside this `bob-cli` workspace in the chezmoi-managed config, so I will keep those
  edits narrowly scoped to the Pomodoro keymap parser and its spec.
