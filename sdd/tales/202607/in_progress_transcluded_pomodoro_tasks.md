---
create_time: 2026-07-07 17:09:11
status: done
prompt: sdd/prompts/202607/in_progress_transcluded_pomodoro_tasks.md
---
# Plan: Close Transcluded In-Progress Tasks with Ctrl+Enter

## Goal

Update the Obsidian `<Ctrl+Enter>` task/Pomodoro completion behavior so embedded transcluded task targets with the
in-progress status `[/]` are closed when the recursive forced-completion path runs.

The source of truth for the implementation is the linked `bob-plugins` repo, in `plugins/task-status-cycler/main.js`. Do
not edit deployed vault plugin files under `~/bob/.obsidian/plugins/` directly; deploy source changes with
`bob plugins sync -p task-status-cycler`.

## Context Reviewed

- Read Obsidian long-term memory with `sase memory read obsidian.md` because this touches vault/Obsidian task behavior.
- Opened the linked `bob-plugins` repo with `sase workspace open -p bob-plugins`.
- Inspected `plugins/task-status-cycler/main.js`, which owns the Vim `<C-CR>` / `<C-Enter>` mapping.
- Reviewed prior SDD plans for:
  - transcluded Ctrl+Enter task completion;
  - Pomodoro close-and-create behavior;
  - recursive transcluded Pomodoro task completion;
  - recursive Ctrl+Enter on Pomodoro transcluded task lines.
- No `bob-cli` subcommands or options are being added, so `memory/cli_rules.md` is not required.

## Current State

The current implementation does not support transcluded in-progress tasks in the recursive completion path.

- `isOpenDoneTaskStatus()` accepts only `[ ]` and `[x]`.
- `resolveTranscludedTaskLineFromSourceText()` rejects any resolved transcluded target whose task status is not accepted
  by `isOpenDoneTaskStatus()`. A `[/]` target never resolves as a task target.
- `completeResolvedTranscludedTaskTargetTree()` traverses descendants for resolved targets, but only writes a target
  when `resolvedTarget.taskStatus.symbol === " "`.
- `getNextTranscludedTaskLineText(..., "x")` also rejects forced completion unless `isOpenDoneTaskStatus(taskStatus)` is
  true.

As a result, recursive Pomodoro/transcluded-line completion closes `[ ]` targets, traverses `[x]` targets, and skips
`[/]` targets entirely.

## Product Decisions

1. Treat `[/]` as closable only in transcluded forced-completion flows.
   - A transcluded source task with `[ ]` or `[/]` should become `[x]`.
   - A transcluded source task already at `[x]` should stay done but still be traversed for descendants.
   - Canceled `[-]`, blocked `[B]`, arbitrary custom statuses, and non-task block targets should remain skipped unless a
     later request broadens the contract.

2. Preserve the existing local open/done toggle semantics.
   - Do not redefine `isOpenDoneTaskStatus()` to include `/`, because it is used by direct current-line toggles.
   - `<Ctrl+Enter>` on a local in-progress task line should not silently become a normal open/done toggle as part of
     this change.

3. Keep recursion semantics unchanged except for accepted target status.
   - Follow embedded transclusions only.
   - Preserve cycle/duplicate guards, same-file child-link rebasing through `originPath`, best-effort error handling,
     and final line/block-ID revalidation before writes.
   - Continue to add/replace `[completion:: YYYY-MM-DD]` for `#task` source lines and keep trailing block IDs final.

4. Apply the same forced-completion status policy wherever the recursive transcluded completion helper is used.
   - Pressing `<Ctrl+Enter>` on an open Pomodoro task should close embedded `[/]` source tasks under it.
   - Pressing `<Ctrl+Enter>` directly on a Pomodoro sub-bullet embedded transclusion should close a selected `[/]`
     target tree.

## Implementation Approach

1. Add focused status predicates near the existing task-status helpers.
   - Add a helper such as `isTranscludedCompletionTraversableStatus(taskStatus)` that accepts `[ ]`, `[/]`, and `[x]`.
   - Add a helper such as `isTranscludedCompletionClosableStatus(taskStatus)` that accepts `[ ]` and `[/]`.
   - Keep `isOpenDoneTaskStatus()` unchanged for direct toggles.

2. Relax transcluded target resolution for recursive forced completion without changing generic open/done toggles.
   - If `resolveTranscludedBlockTarget()` currently needs to serve both generic toggling and forced recursive
     completion, add an option or a sibling resolver that can accept the traversable status predicate.
   - Generic direct transcluded toggles outside the Pomodoro recursive path should still resolve only open/done targets.
   - Recursive forced-completion callers should resolve `[ ]`, `[/]`, and `[x]` targets so done parents and in-progress
     parents can expose descendants.

3. Update recursive forced-completion writes.
   - In `completeResolvedTranscludedTaskTargetTree()`, replace the open-only write check with the closable predicate.
   - Force both `[ ]` and `[/]` to `"x"`.
   - Keep already-done `[x]` targets as traversal-only, with no rewrite.

4. Update forced transcluded line rewriting.
   - In `getNextTranscludedTaskLineText()`, when `forcedNextSymbol === "x"`, allow source statuses accepted by the new
     closable predicate, not just `isOpenDoneTaskStatus()`.
   - Preserve the existing no-op when the source line already has the forced target symbol.
   - Leave non-forced toggling on `getNextOpenDoneSymbol()` unchanged.

5. Keep code review surface narrow.
   - Do not alter Pomodoro placeholder creation, carry-forward bullet copying, cursor movement, centering, or
     non-embedded link behavior.
   - Do not edit unrelated plugins.
   - After source changes, run `bob plugins sync -p task-status-cycler` from the source repo and verify the deployed
     vault copy matches the intended source diff.

## Acceptance Criteria

- A Pomodoro with an embedded transcluded target whose source line is `- [/] #task ... ^a` marks that source line done
  when `<Ctrl+Enter>` completes the Pomodoro.
- The completed `#task` source line gains or updates `[completion:: YYYY-MM-DD]` using the existing metadata placement
  rules.
- A recursive chain `A [/] -> B [/] -> C [ ]` closes A, B, and C.
- A done parent with an in-progress embedded child leaves the parent done and closes the child.
- In-progress targets are traversed for child embedded transclusions before or while being closed, matching the existing
  recursive behavior for open/done targets.
- Canceled, blocked, arbitrary custom statuses, non-task blocks, unresolved links, and plain non-embedded block links
  remain skipped.
- Generic direct current-line open/done toggles still treat only `[ ]` and `[x]` as toggleable.
- Direct transcluded-line toggles outside the Pomodoro recursive context keep their existing open/done behavior unless
  implementation review shows the user explicitly wants that path broadened too.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused Node checks with the existing helper exports and a stubbed Obsidian app:

- Status predicates classify `[ ]`, `[/]`, `[x]`, `[-]`, and `[B]` as expected.
- Recursive forced completion closes a single `[/]` transcluded source task.
- Recursive forced completion closes a mixed `[ ]` / `[/]` / `[x]` chain while traversing done parents.
- A `[/]` source task with an embedded child is traversed and closed.
- Forced completion still skips `[-]`, `[B]`, non-task blocks, and unresolved targets.
- Non-forced transcluded toggling still only toggles open/done targets.
- Existing Pomodoro completion plan behavior remains stable.

Manual smoke test after `bob plugins sync -p task-status-cycler` and Obsidian plugin reload:

1. Create a scratch Pomodoro with an embedded source task currently marked `[/]`.
2. Press `<Ctrl+Enter>` on the Pomodoro line; confirm the transcluded source task becomes `[x]` with completion
   metadata.
3. Repeat with a `[/]` source task that embeds another `[/]` task.
4. Press `<Ctrl+Enter>` directly on the embedded Pomodoro sub-bullet; confirm the selected tree closes recursively.
5. Confirm a normal local `[/]` task line does not start toggling through the direct open/done path.

## Risks and Mitigations

- Reusing `isOpenDoneTaskStatus()` for the new behavior would accidentally broaden direct local toggles. Mitigate with
  separate predicates for recursive forced completion.
- Resolver changes could unintentionally broaden non-recursive transcluded toggles. Mitigate with an explicit option or
  separate resolver path for forced recursive completion.
- Same-file descendant links are sensitive to origin handling. Keep the existing `originPath` rebasing unchanged and
  include a same-file descendant smoke test.
