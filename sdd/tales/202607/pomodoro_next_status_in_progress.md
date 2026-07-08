---
create_time: 2026-07-08 14:08:29
status: wip
prompt: sdd/prompts/202607/pomodoro_next_status_in_progress.md
---
# Plan: `<Ctrl+Enter>` closing a Pomodoro should start `[*]` Next tasks (mark them in-progress `[/]`)

## Summary

When the `<Ctrl+Enter>` keymap closes a Pomodoro in a daily note, the `task-status-cycler` Obsidian plugin is supposed
to mark every Obsidian task reached through a **bare non-transcluded block link** (`[[note#^id]]`) in that Pomodoro's
sub-bullets as in-progress (`[/]`). This works for tasks that are currently open (`[ ]`), but **silently skips tasks
that carry the newly-introduced `[*]` "Next" status** — they stay `[*]` instead of becoming `[/]`.

The root cause is that the two status predicates guarding the non-transcluded "start" flow were written before the `[*]`
Next status existed, so they do not recognize `[*]` as an open/startable status. The fix teaches those two predicates to
treat `[*]` exactly like `[ ]` in this flow. It is a small, plugin-only behavior change.

The source of truth is the linked `bob-plugins` repo, in `plugins/task-status-cycler/main.js`. Do not edit deployed
vault plugin files under the vault's `.obsidian/plugins/` directly; deploy the source change with
`bob plugins sync -p task-status-cycler` from the linked source repo after implementation.

## Background — how `<Ctrl+Enter>` starts non-transcluded Pomodoro links today

A Pomodoro entry is an indent-0 checkbox ledger line (a time range or `()` placeholder) under a `## Pomodoros` heading.
Its sub-bullets may contain bare non-transcluded task links like `- [[project#^task-id]]`. Closing the Pomodoro with
`<Ctrl+Enter>` is supposed to "start" (force to `[/]`) the linked task for each such bare link.

Two entry points feed the same non-transcluded start machinery, and both exhibit the bug:

1. **Closing the Pomodoro entry** — `completeActivePomodoroTask()` classifies the sub-bullets and calls
   `startPomodoroNonTranscludedTaskBullets()`, which calls `startNonTranscludedTaskTarget()` for each bare-link target.
2. **`<Ctrl+Enter>` directly on a bare non-transcluded sub-bullet** — `startActivePomodoroNonTranscludedTaskLine()`.

Both resolve the link's target task line via `resolveTranscludedBlockTarget()` and then decide whether to force it to
`[/]` via `startResolvedNonTranscludedTaskTarget()`. The forced write is finalized by
`replaceResolvedTranscludedTaskLine()` → `getNextTranscludedTaskLineText()` → `canForceTranscludedTaskStatus()`.

## Root cause

The non-transcluded start flow gates a target task through two status predicates, and **neither recognizes the `[*]`
Next status** (both predate it — before `[*]` existed, every open task was `[ ]`, so these predicates covered all open
tasks):

1. **Resolution gate** — `isNonTranscludedStartResolvableStatus(taskStatus)` accepts only `" "`, `"/"`, `"x"`. It is
   passed as the `taskStatusPredicate` when resolving the target. Inside `resolveTranscludedTaskLineFromSourceText()`, a
   target whose status fails this predicate causes resolution to return `null`. A `[*]` target therefore **never
   resolves at all** — the link is treated as if it points at nothing, so the task is silently skipped. **This is the
   primary blocker.**

2. **Startable / force-write gate** — `isNonTranscludedStartableStatus(taskStatus)` accepts only `" "`. It is used in
   two places: (a) `startResolvedNonTranscludedTaskTarget()` decides whether to write `[/]`, and (b)
   `canForceTranscludedTaskStatus()` (for `forcedNextSymbol === "/"`) is the final revalidation before the forced write.
   Even if a `[*]` target were resolvable, this gate would still refuse to write `[/]`.

Because `[*]` is in neither set, a Next task linked from a Pomodoro sub-bullet is passed over when the Pomodoro is
closed. This is exactly the regression the user observed after the recent addition of the `[*]` Next status (the
"Blocked → Next" migration and the `next_status_on_pomodoro_task_link` feature, which itself promotes Pomodoro-linked
open tasks to `[*]`). The two features now interact: a task promoted to `[*]` via a `^^` link can no longer be started
to `[/]` when its Pomodoro is closed.

## Desired behavior

In the non-transcluded Pomodoro-link start flow, treat `[*]` (Next) as an open, startable status — force it to `[/]`
in-progress just like `[ ]`:

- `[ ]` proper `#task` target → `[/]` (unchanged).
- `[*]` proper `#task` target → `[/]` (**new — the fix**).
- `[/]` target → resolves but is not rewritten; stays `[/]` (idempotent, unchanged).
- `[x]` target → resolves but is not rewritten; stays `[x]` (unchanged).
- `[-]`, `[B]`, arbitrary custom statuses, non-`#task` checkboxes, non-task blocks, unresolved/malformed links → skipped
  without aborting sibling links (unchanged — do **not** broaden these).

Both entry points (closing the Pomodoro entry, and `<Ctrl+Enter>` directly on a bare non-transcluded sub-bullet) must
pick up the new behavior. All existing guards (line/block-ID revalidation, best-effort per-link error isolation,
same-file editor vs. vault write path, seen-set dedupe) stay intact.

## Implementation approach

Confine the change to `plugins/task-status-cycler/main.js` in the `bob-plugins` linked repo. Add `[*]` to the two status
predicates so it flows through every call site automatically:

1. **`isNonTranscludedStartResolvableStatus`** — additionally accept `taskStatus.symbol === "*"` so a `[*]` target
   resolves (used at both non-transcluded resolution call sites: the Pomodoro-completion path and the direct-line path).

2. **`isNonTranscludedStartableStatus`** — accept `" "` **or** `"*"` so a resolved `[*]` target is written to `[/]`.
   This one predicate governs both the write decision in `startResolvedNonTranscludedTaskTarget()` and the final
   forced-write revalidation in `canForceTranscludedTaskStatus()` (for the `"/"` forced symbol), so both are fixed at
   once.

No new write code is needed: the existing write path `rewriteTaskLineForTranscludedSource(lineText, "/", …)` swaps only
the status character (and strips any `[completion::]` field for `#task` lines), so `[*]` → `[/]` behaves identically to
`[ ]` → `[/]` and preserves the trailing block ID and all other line content.

Deliberately do **not** touch the parallel predicates for other flows (`isOpenDoneTaskStatus`,
`isTranscludedCompletionTraversableStatus`, `isTranscludedCompletionClosableStatus`, `isCyclableTaskStatus`) — the
report is specifically about the non-transcluded in-progress start, and broadening the others would change unrelated
toggle / recursive-completion behavior.

## Acceptance criteria

- Closing an open Pomodoro whose sub-bullet is `- [[project#^a]]`, where `^a` is `- [*] #task … ^a`, rewrites only `^a`
  to `- [/] #task … ^a`, preserving its block ID and any other body content.
- The same holds when `<Ctrl+Enter>` is pressed directly on the bare non-transcluded sub-bullet.
- `[ ]` targets still become `[/]`; `[/]` and `[x]` targets are still left unchanged (idempotent).
- `[-]`, `[B]`, arbitrary custom statuses, non-`#task` checkbox lines, non-task blocks, and unresolved/malformed links
  are still skipped without aborting the rest of the Pomodoro completion.
- A Pomodoro with several eligible bare-link sub-bullets still starts each independent root; the non-recursive contract
  (resolved target treated as a leaf; its descendants are not scanned) is unchanged.
- Embedded transcluded (`![[…#^id]]`) recursive forced-completion behavior is unchanged.
- Carry-forward bullet copying, placeholder creation, cursor placement, and centering are unchanged.

## Verification plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused Node checks using the existing helper exports (`module.exports.helpers`) and a stubbed Obsidian app:

- `isNonTranscludedStartResolvableStatus` classifies `[ ]`, `[*]`, `[/]`, `[x]` as resolvable and `[-]`, `[B]`, custom
  as not resolvable.
- `isNonTranscludedStartableStatus` returns true for `[ ]` and `[*]`, false for `[/]`, `[x]`, `[-]`, `[B]`, custom.
- Full non-transcluded start writes `[/]` for a `[*]` proper `#task` target and leaves `[/]`/`[x]` targets unchanged.
- A `[*]` target with descendant bare links has its descendants ignored (leaf semantics preserved).
- Non-`#task` `[*]` checkboxes and non-task blocks are skipped.
- Block ID and remaining line content are preserved across the `[*]` → `[/]` rewrite.

Manual smoke test after `bob plugins sync -p task-status-cycler` and an Obsidian plugin reload:

1. In a daily note, create an open Pomodoro with a sub-bullet `- [[source#^a]]` where `^a` is a `[*]` `#task`.
2. Press `<Ctrl+Enter>` to close the Pomodoro; confirm `^a` becomes `[/]` and keeps its block ID; the bullet is copied
   forward as today.
3. Repeat pressing `<Ctrl+Enter>` directly on the bare sub-bullet; confirm the linked `[*]` task becomes `[/]`.
4. Confirm `[/]` and `[x]` targets stay unchanged, and `[-]`/custom targets are skipped.
5. Confirm embedded transcluded Pomodoro links still close their trees to `[x]` recursively.

## Rollout / operational notes

- Edit only the source in the `bob-plugins` linked repo (`plugins/task-status-cycler/main.js`). The deployed copy under
  the vault's `.obsidian/plugins/` is overwritten on sync.
- Deploy from the linked-repo checkout with `bob plugins sync -p task-status-cycler` (in a SASE workspace the default
  source path does not exist, so pass `-r "$PWD"` from the checkout; use `--dry-run` first, then without it). Reload the
  plugin in Obsidian afterward, and confirm the deployed `main.js` matches the source.
- Committing the `bob-plugins` change is left to the normal per-repo commit flow and is not performed as part of
  implementation unless requested.

## Out of scope

- Defining/renaming any status, CSS/styling, or Tasks-plugin config — the `[*]` Next status already exists.
- The bob-cli Rust code, docs, or fixtures — this is a plugin-only behavior change.
- Embedded transcluded recursive completion, direct current-line open/done toggles, and status cycling — untouched.
- Broadening which statuses the other (non-start) predicates accept.
