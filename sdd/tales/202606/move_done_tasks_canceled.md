---
create_time: 2026-06-07 06:50:54
status: done
---
# Plan: Ensure move-done-tasks Handles Canceled Tasks

## Context

`bob move-done-tasks` is implemented in `src/native/collect_done.rs`. The command scans Markdown notes under `BOB_DIR`,
excludes archive/config directories, builds a per-file collection plan, moves completed task blocks into
`done/..._done.md`, adds or repairs `done_tasks` source metadata, repairs archive metadata, repairs links to moved block
ids, and commits/pushes touched vault files when the vault is a git worktree.

The threshold decision is made in `build_collection_plan()`:

- read each Markdown source note
- call `transform_markdown()`
- set `moves_tasks = transform.task_count >= threshold`
- only when `moves_tasks` is true, write the transformed source and append moved task blocks to the archive

`transform_markdown()` increments `task_count` for every line accepted by `collectible_task_line()`. Current inspection
shows `collectible_task_line()` already accepts `[x]`, `[X]`, and `[-]` checkbox markers when the line is a Markdown
list item and contains a bounded `#task` tag. Existing unit coverage includes a parser-level test for done/canceled
recognition and a mixed done+canceled threshold test in `build_collection_plan()`.

The requested behavior is therefore: a canceled task line must be indistinguishable from a done task line for collection
eligibility and movement. A file with only canceled task blocks should meet threshold and move. A mixed file should meet
threshold based on the combined done+canceled count. Below-threshold canceled tasks should remain in place.

## Implementation Strategy

1. Strengthen the behavioral contract around task status recognition.
   - Keep the accepted completed statuses as `[x]` and `[X]`.
   - Keep the accepted canceled status as `[-]`.
   - Keep existing requirements that a moved task line must be a Markdown list item and contain a real `#task` tag.
   - Do not add new CLI options, subcommands, output sections, or archive naming rules.

2. Add focused regression coverage for threshold behavior.
   - Add a `build_collection_plan()` unit test for a source note whose threshold is met by canceled-only tasks.
   - Assert the source note is emptied except for `done_tasks` frontmatter, the archive receives the canceled blocks,
     and `task_count` equals the number of canceled blocks.
   - Add a below-threshold canceled-only assertion so `[-]` is counted but still respects the configured threshold.
   - Keep or lightly adjust the existing mixed done+canceled threshold test so it explicitly documents combined
     counting.

3. Add command-level regression coverage if practical within the existing CLI test helpers.
   - Prefer a lightweight integration test that runs `bob move-done-tasks --threshold=2` against a temp non-git vault
     with two canceled `#task` lines and one active task.
   - Assert the command exits successfully, reports one file meeting threshold and two moved task blocks, writes the
     expected source and archive files, and leaves active tasks untouched.
   - Use non-git mode to avoid remote/git setup when the behavior under test is parser/planner movement, not git.

4. Audit and patch implementation only if the new tests expose a gap.
   - If canceled-only threshold movement already passes, avoid unnecessary production-code churn.
   - If any path still treats only `[x]` as moveable, route that path through the same `collectible_task_line()` status
     logic or equivalent shared predicate so done and canceled task blocks cannot diverge again.
   - Preserve block-id extraction, archive block-id deduplication, metadata repair, and link-repair behavior for
     canceled blocks because those are downstream of the same moved archive append.

5. Validate.
   - Run `cargo fmt --check`.
   - Run focused tests: `cargo test collect_done`.
   - Run the relevant CLI integration test(s) for `move-done-tasks`.
   - If the focused surface is stable and time permits, run the full test suite.

## Risks And Mitigations

- Risk: the code already supports `[-]`, so a production-code change could create needless churn. Mitigation: make tests
  drive the work and leave implementation untouched if the behavior is already correct.
- Risk: canceled-only behavior may be covered at parser level but not at command level. Mitigation: add at least one
  end-to-end command regression using a temp vault.
- Risk: changing task recognition could accidentally collect active, in-progress, malformed, or non-`#task` checkboxes.
  Mitigation: keep the existing negative cases and avoid broadening status parsing beyond the requested `[-]` support.

## Done Criteria

- `bob move-done-tasks` treats canceled `[-] #task` blocks the same as completed `[x]`/`[X] #task` blocks for threshold
  counting and movement.
- A canceled-only note moves when its canceled task count meets threshold.
- A mixed done+canceled note moves when the combined count meets threshold.
- Canceled task blocks below threshold remain in the source note.
- Focused Rust unit tests and command-level regression tests pass.
