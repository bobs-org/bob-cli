---
create_time: 2026-06-20 15:30:15
status: wip
prompt: sdd/prompts/202606/recursive_transcluded_line_ctrl_enter.md
---
# Plan: Recursive Ctrl+Enter on Pomodoro Transcluded Task Lines

## Goal

Extend the existing Obsidian `<Ctrl+Enter>` behavior so that pressing the keymap directly on an embedded transcluded
task block link that is a sub-bullet of the current Pomodoro task closes that transcluded task tree recursively.

Today, pressing `<Ctrl+Enter>` on the Pomodoro task line recursively forces all embedded transcluded task links under
that Pomodoro to done. Pressing `<Ctrl+Enter>` directly on one of those embedded sub-bullet lines still goes through the
older direct-transclusion toggle path and only touches the one referenced source task. This plan brings that direct
Pomodoro sub-bullet case in line with the recursive Pomodoro completion behavior.

## Context Reviewed

- Read Obsidian long-term memory with:
  `sase memory read obsidian.md --reason "Need Obsidian vault and keymap workflow context before changing ctrl-enter task behavior"`.
- Read the SASE planning skill instructions and am submitting this plan before any code changes.
- Read `bob-plugins` local instructions:
  `/home/bryan/.local/state/sase/workspaces/bobs-org/bob-plugins/bob-plugins_10/AGENTS.md`. This repo is the source of
  truth for Bryan's custom Obsidian plugins and must be deployed to the vault with `bob plugins sync` after source
  changes.
- Inspected the live plugin: `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
- Compared plugin workspaces. `bob-plugins_10` is clean, latest-used, and byte-identical to the live
  `task-status-cycler/main.js`; `bob-plugins_12` and `bob-plugins_13` are older and do not contain the recent recursive
  Pomodoro code. Use:
  `/home/bryan/.local/state/sase/workspaces/bobs-org/bob-plugins/bob-plugins_10/plugins/task-status-cycler/main.js`.
- Reviewed prior SDD plans:
  - `sdd/tales/202606/transcluded_ctrl_enter_tasks.md`
  - `sdd/tales/202606/recursive_transcluded_pomodoro_tasks_1.md`
- No `bob-cli` subcommands or options are being added, so `memory/cli_rules.md` is not required.

## Current State

The relevant flow is in `plugins/task-status-cycler/main.js`:

- `<C-CR>` and `<C-Enter>` both call `handleVimTaskToggleOpenDone()`.
- If the active line is an open top-level Pomodoro task, `handleVimTaskToggleOpenDone()` calls
  `completeActivePomodoroTask()`.
- `completeActivePomodoroTask()` classifies the Pomodoro's sub-bullets, calls
  `completePomodoroTranscludedTaskBullets()`, then builds and applies the local Pomodoro completion plan.
- `completePomodoroTranscludedTaskBullets()` now uses `completeTranscludedTaskTargetTree()` with a shared seen-set. That
  recursive helper:
  - resolves embedded block links relative to `context.originPath`;
  - traverses descendant embedded transclusions under the resolved task's list-item block;
  - uses `resolved file path + block ID` to stop cycles and duplicate processing;
  - forces open source tasks to done;
  - leaves already-done source tasks done but still traverses their descendants.
- If the active line is not a direct task line, `handleVimTaskToggleOpenDone()` falls through to
  `toggleActiveTranscludedTaskOpenDone()`.
- `toggleActiveTranscludedTaskOpenDone()` currently resolves the active line's embedded block link and calls
  `replaceResolvedTranscludedTaskLine()` once. Its inline comment explicitly says this direct path stays non-recursive.

That last point is the behavior this request supersedes for embedded transcluded lines that are Pomodoro sub-bullets.

## Behavioral Contract

1. Only Pomodoro sub-bullet transcluded lines get the new recursive forced-done behavior.
   - The active line must be inside the `## Pomodoros` section.
   - The active line must be in the indented sub-bullet block belonging to a top-level Pomodoro task line.
   - The active line must contain a supported embedded block transclusion such as `![[note#^id]]` or `![[#^id]]`.
   - Direct transcluded-line Ctrl+Enter outside that Pomodoro-sub-bullet context keeps today's single-target toggle.

2. The direct Pomodoro sub-bullet path should mirror the existing Pomodoro recursive completion semantics for the
   selected embedded target tree.
   - Open source tasks in the selected tree become done.
   - Already-done source tasks stay done and their embedded descendants are still traversed.
   - Other statuses, non-task block targets, unresolved files, and broken links are skipped best-effort.
   - Plain non-embedded `[[note#^id]]` links are not followed.

3. This action does not complete or restructure the local Pomodoro itself.
   - Pressing `<Ctrl+Enter>` on the Pomodoro task line still completes the Pomodoro, creates/jumps to the next Pomodoro
     as today, and processes every embedded sub-bullet under it.
   - Pressing `<Ctrl+Enter>` on one embedded sub-bullet line only closes that selected transcluded task tree.
   - It should not create a new Pomodoro placeholder, carry forward bullets, or move the cursor to the next Pomodoro.

4. Ambiguity handling should remain conservative.
   - Reuse the existing "single candidate or cursor-contained candidate" selection logic.
   - If a line has multiple embedded block transclusions and the cursor does not identify exactly one, no-op.

5. Reopening behavior remains available outside the Pomodoro sub-bullet recursive path.
   - The old direct transcluded-line toggle remains the fallback for non-Pomodoro contexts.
   - Within a Pomodoro sub-bullet, the behavior is intentionally completion-oriented: a done target is not reopened by
     this path because that would conflict with "recursive completion" and with the existing done-parent traversal rule.

## Implementation Approach

1. Work in the source plugin checkout.
   - Use `/home/bryan/.local/state/sase/workspaces/bobs-org/bob-plugins/bob-plugins_10`.
   - Re-check `git status --short` before editing.
   - Do not edit `~/bob/.obsidian/plugins/task-status-cycler/main.js` directly.

2. Add a focused Pomodoro sub-bullet context detector.
   - Add a helper such as `getActivePomodoroTranscludedTaskLineTarget(editor, activePath)`.
   - Reuse `getActiveLineTranscludedTaskTarget()` to parse and disambiguate the active embedded transclusion.
   - Read editor lines with `getEditorLineTexts(editor)` and locate `findPomodorosSectionInLines(lines)`.
   - Confirm the active line is inside the Pomodoros section and is an indented list line.
   - Find the owning top-level Pomodoro task above the active line, then validate the active line falls inside
     `getSubBulletBlockRange(lines, pomodoroLine, section)`.
   - Return the selected candidate plus the owning Pomodoro line if useful for tests/debugging.

3. Refactor the recursive helper enough for both callers.
   - Keep `completeTranscludedTaskTargetTree(candidate, context, seen, depth)` available for the existing Pomodoro-task
     flow.
   - Add an internal resolved-target variant, for example
     `completeResolvedTranscludedTaskTargetTree(resolvedTarget, context, seen, depth)`, so a direct sub-bullet caller
     can resolve once, know that the command was handled, and avoid falling through to the old toggle path when the root
     is already done.
   - Have the inner helper return a small result like `{ visited, changed }` or an equivalent value. Existing callers
     can ignore the exact result; the direct sub-bullet path needs to distinguish "resolved and traversed, even if no
     write" from "not applicable".
   - Preserve the current traversal order, cycle/depth/target guards, same-file link rebasing via `originPath`, and
     final line revalidation before writes.

4. Add a new command path before the generic direct-transclusion toggle.
   - In `handleVimTaskToggleOpenDone()`, keep direct open Pomodoro task handling first.
   - Keep direct open/done task-line toggling second.
   - Before falling through to `toggleActiveTranscludedTaskOpenDone()`, attempt a new method such as
     `completeActivePomodoroTranscludedTaskLine(editor, activeFile)`.
   - That method should:
     - get the active Pomodoro transcluded-line candidate;
     - build `{ editor, activePath, originPath: activePath }`;
     - resolve the target;
     - run the resolved recursive forced-done helper with a fresh seen-set;
     - return true when the target was resolved and traversal was attempted, even if every task was already done.
   - If the active line is not a Pomodoro sub-bullet transclusion or cannot be resolved, fall back to the existing
     `toggleActiveTranscludedTaskOpenDone()` behavior.

5. Keep source writes and deployment disciplined.
   - Use existing `replaceResolvedTranscludedTaskLine()`, editor-buffer handling for active-file writes, and vault
     `process`/`read`/`modify` handling for other files.
   - After source changes, run `bob plugins sync -p task-status-cycler` from the source repo to deploy to `~/bob`.
   - Inspect source and deployed diffs separately before finishing.

## Acceptance Criteria

- With a Pomodoro like:

  ```md
  - [ ] (10:00-10:25) Work
    - ![[project#^a]]
  ```

  and source task `^a` containing a child embedded task link to `^b`, pressing `<Ctrl+Enter>` on the `![[project#^a]]`
  line marks both `^a` and `^b` done.

- Multi-level chains such as A -> B -> C close every open task in the selected embedded tree.
- A done root target with an open embedded descendant leaves the root done and closes the descendant.
- Cycles and duplicate links terminate without repeated writes.
- Same-file child links inside external source notes, such as `![[#^child]]`, resolve relative to that external note.
- Plain non-embedded block links, non-task blocks, unresolved links, and non-open/done statuses are skipped without
  aborting the command.
- Pressing `<Ctrl+Enter>` on the Pomodoro task line still behaves exactly as it does today.
- Pressing `<Ctrl+Enter>` on a transcluded line outside a Pomodoro sub-bullet still uses the existing single-target
  open/done toggle.
- No source edits are made directly under `~/bob/.obsidian/plugins/`; deployed changes come from `bob plugins sync`.

## Verification Plan

Static checks from `bob-plugins_10`:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused Node checks with a stubbed `obsidian` module, fake app/vault/metadata cache, and fake editors:

- Pomodoro sub-bullet detector returns a candidate for an embedded transcluded line under a top-level Pomodoro.
- Detector rejects embedded transcluded lines outside `## Pomodoros`, top-level non-Pomodoro lines, and non-embedded
  block links.
- Direct Pomodoro sub-bullet completion closes A -> B -> C.
- Done parent with open child closes the child and does not reopen the parent.
- Duplicate and cyclic references are visited once and terminate.
- Same-file child links inside an external source file resolve against that source file.
- Ambiguous multi-embed lines no-op unless the cursor is inside exactly one embed.
- Non-Pomodoro direct transcluded-line Ctrl+Enter still toggles only one target.
- Existing Pomodoro-task-line completion plan behavior remains stable.

Manual smoke test after `bob plugins sync -p task-status-cycler` and an Obsidian plugin reload:

1. Create a scratch daily-style note with a Pomodoro and one embedded transcluded task sub-bullet.
2. Give that source task one nested embedded child task, plus a same-file child link case.
3. Press `<Ctrl+Enter>` directly on the embedded sub-bullet line and confirm the selected tree is done.
4. Repeat with a done parent/open child.
5. Press `<Ctrl+Enter>` on the Pomodoro line and confirm full Pomodoro completion still works.
6. Press `<Ctrl+Enter>` on a transcluded line outside a Pomodoro sub-bullet and confirm the old toggle behavior remains.

## Risks and Mitigations

- The main behavioral risk is accidentally making every direct transcluded-line toggle recursive. Mitigate by adding an
  explicit Pomodoro sub-bullet detector and falling back to the existing toggle outside that context.
- A done root target returning "no write" could accidentally fall through and reopen through the old toggle path.
  Mitigate by returning a handled/visited result from the recursive path, not just a changed boolean.
- Same-file descendant links are easy to resolve against the wrong note. Preserve the existing `originPath` rebasing
  from the recursive Pomodoro implementation and cover it in focused checks.
- The plugin repo has no formal test runner. Use syntax/manifest/diff checks, focused Node harness checks, and an
  Obsidian smoke test after deployment.
