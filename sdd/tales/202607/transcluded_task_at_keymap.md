---
create_time: 2026-07-07 19:15:59
status: wip
prompt: sdd/prompts/202607/transcluded_task_at_keymap.md
---
# Plan: Conditional `@` Keymap for Transcluded Task Starts

## Goal

Add a Vim normal-mode `@` key behavior in Obsidian that starts the task targeted by the current line's transcluded block
link by changing its source task checkbox from `[ ]` to `[/]`.

The key must preserve Vim macro playback everywhere outside the intended context. In particular, do not register a
global `vim.mapCommand("@", ...)` mapping that would steal the normal-mode macro command on unrelated lines.

## Context Reviewed

- Read Obsidian long-term memory through `sase memory read obsidian.md` because this changes Obsidian plugin/task
  behavior.
- Opened the linked `bob-plugins` source repo with `sase workspace open -p bob-plugins`, as required for linked-repo
  code review.
- Inspected `plugins/bob-navigation-hotkeys/main.js`, which owns the existing `!` line-transclusion command.
- Inspected `plugins/task-status-cycler/main.js`, which owns task status mutation, transcluded block-target resolution,
  and the existing `<Ctrl+Enter>` task/Pomodoro paths.
- Reviewed recent SDD plans for transcluded Ctrl+Enter task handling, recursive transcluded Pomodoro completion,
  in-progress transcluded tasks, non-transcluded Pomodoro link starts, and the original `!` transclusion toggle.
- No `bob-cli` subcommands or options are being added, so `memory/cli_rules.md` is not required.

## Current State

- `bob-navigation-hotkeys` exposes `toggle-line-transclusions`, and its helper toggles `!` markers for recognized links
  on the current line. This is line/link syntax behavior, not task-state behavior.
- `task-status-cycler` already has the task-aware pieces needed for this change:
  - `getActiveLineTranscludedTaskTarget(editor, sourcePath)` recognizes an unambiguous embedded block transclusion such
    as `![[note#^id]]` or same-file `![[#^id]]`.
  - `resolveTranscludedBlockTarget(candidate, context, options)` resolves the target file and block line.
  - `replaceResolvedTranscludedTaskLine(resolvedTarget, context, "/")` can force an open target to `[/]`.
  - `canForceTranscludedTaskStatus(..., "/")` already allows only open `[ ]` targets through
    `isNonTranscludedStartableStatus()`.
- Existing forced `[/]` behavior removes stale completion metadata for `#task` lines and preserves trailing block IDs.
- The plugin already uses narrow capture-phase keydown listeners for Vim-mode chords that cannot be safely expressed as
  simple Vim mappings.

## Product Decisions

1. Implement the `@` behavior in `task-status-cycler`.
   - This key changes a task status, so it belongs beside the existing transcluded task resolver/writer rather than in
     the line-transclusion plugin.
   - `bob-navigation-hotkeys` should not need changes unless implementation exposes a missing reusable helper.

2. Do not use `vim.mapCommand("@", ...)`.
   - `@` is Vim's macro playback prefix.
   - A Vim mapping would be active globally in normal mode and would break macros even when the handler no-ops.
   - Use a capture-phase physical keydown listener that returns early without preventing default unless the current line
     visibly has an eligible embedded block transclusion.

3. Define key activation narrowly and synchronously.
   - Intercept only editor keydown events for literal `@` in Vim normal mode, with no Ctrl/Alt/Meta modifiers.
   - Require an active Markdown editor and an event target inside that editor.
   - Require `getActiveLineTranscludedTaskTarget()` to find an unambiguous embedded block transclusion on the active
     line. A single transcluded block link may be used from anywhere on the line; multiple links require the cursor to
     be inside exactly one candidate.
   - Non-transcluded block links like `[[note#^id]]`, ordinary note embeds without `#^id`, ambiguous multi-link lines,
     insert/visual/replace mode, and non-editor targets must fall through to Vim untouched.

4. Keep the task-state operation one-way.
   - If the resolved source task is open `[ ]`, write it as in-progress `[/]`.
   - If the target is already `[/]`, done `[x]`, canceled, blocked, custom, unresolved, or not a task line, do not
     rewrite it.
   - This matches the requested "open to in-progress" behavior and avoids using `@` as a full status cycle.

5. Prefer silence over noisy notices.
   - Ineligible contexts must fall through to macro playback and should not show notices.
   - If the visible transcluded block link is intercepted but later fails async resolution or is not open, silently
     no-op unless a targeted debug notice proves useful during implementation. This keeps `@` lightweight.

## Implementation Approach

1. Add a dedicated `@` input listener in `plugins/task-status-cycler/main.js`.
   - Add `registerTranscludedTaskStartInputListeners()` modeled on the existing child-bullet and navigation keydown
     listener patterns.
   - Track handled keydown events with a `WeakSet` so window/document capture listeners cannot double-dispatch.
   - Call this registration from `onload()`.

2. Add a narrow key/event predicate.
   - Accept `event.key === "@"`.
   - Reject Ctrl, Alt, and Meta modifiers. Shift is allowed because `@` is often typed with Shift on US keyboards.
   - Ignore non-editor targets through the existing focused Markdown editor guard.
   - Require Vim normal mode via the existing `resolveNormalModeVimCm()` / `getCurrentVimMode()` path.

3. Check activation before consuming the event.
   - Resolve the active Markdown file path.
   - Call `getActiveLineTranscludedTaskTarget(editor, activeFile.path)` before `preventDefault()`.
   - If there is no candidate, return `false` immediately so Vim macros receive `@`.
   - Once a candidate exists, prevent default and stop propagation, then perform the async task-state update.

4. Reuse the existing transcluded task resolver/writer.
   - Add a method such as `startActiveTranscludedTaskLine(editor, activeFile, candidate)` or reuse a smaller helper if
     one already fits cleanly.
   - Build the existing context shape `{ editor, activePath, originPath }`.
   - Resolve the candidate with `taskStatusPredicate: isNonTranscludedStartableStatus`.
   - Consider adding `linePredicate: isProperObsidianTaskLine` only if implementation review confirms this should be
     limited to `#task` source lines. Otherwise keep the broader existing transcluded-task behavior for any Markdown
     task line.
   - Write with `replaceResolvedTranscludedTaskLine(resolvedTarget, context, "/")`.

5. Keep existing behavior untouched.
   - Do not change the `!` transclusion toggle.
   - Do not alter `<Ctrl+Enter>` direct task, Pomodoro, embedded-transclusion completion, or non-transcluded Pomodoro
     start behavior.
   - Do not edit `.obsidian.vimrc` or hotkey JSON.

6. Deploy from source after implementation.
   - Because `bob-plugins` is the source of truth, run `bob plugins sync -p task-status-cycler` after source changes.
   - Verify the deployed vault plugin copy matches the intended source change.

## Acceptance Criteria

- Pressing `@` in Vim normal mode on a line with one unambiguous embedded block transclusion such as
  `- ![[Project#^task-id]]` changes the resolved source task from `- [ ] ... ^task-id` to `- [/] ... ^task-id`.
- Same-file transclusions such as `![[#^task-id]]` work.
- Aliased/foldered block transclusions continue to resolve through the existing resolver.
- Pressing `@` on the same line when the source task is already `[/]` or `[x]` does not rewrite it.
- Pressing `@` on non-transcluded links, non-block embeds, ambiguous multi-transclusion lines where the cursor is not
  inside a unique target, non-task blocks, insert mode, visual mode, replace mode, and non-editor UI leaves Vim macro
  playback untouched.
- Existing `@` macros still work on ordinary normal-mode lines.
- Existing `!`, `<Ctrl+Enter>`, `<Enter>`, Backspace, `o`/`O`, `<C-d>`, and `<C-u>` behavior remains unchanged.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused implementation checks, using helper exports or small stubs as practical:

- The `@` key predicate accepts literal `@` without Ctrl/Alt/Meta and rejects unrelated keys/chords.
- The dispatch path does not call `preventDefault()` when no embedded block transclusion candidate exists.
- A single `![[note#^id]]` candidate is accepted from anywhere on the line.
- Multiple embedded block transclusions require the cursor to be inside exactly one candidate.
- The async start path resolves an open transcluded task and calls the writer with forced symbol `"/"`.
- Already in-progress, done, blocked, canceled, unresolved, and non-task targets do not write.

Manual smoke test after `bob plugins sync -p task-status-cycler` and plugin reload:

1. On a scratch line with `- ![[Some note#^open-task]]`, press `@`; confirm the source task becomes `[/]`.
2. Press `@` again on the same line; confirm it remains `[/]`.
3. Repeat with a same-file `![[#^open-task]]`.
4. On a line with `[[Some note#^open-task]]`, press `@` followed by a macro register; confirm Vim macro playback still
   works and the task is not started.
5. On an ordinary note line, run a known `@` macro and confirm it behaves as before.
6. Confirm `!` still toggles transclusion for the same block-link line.

## Risks and Mitigations

- Risk: consuming `@` too broadly would break Vim macros.
  - Mitigation: no Vim mapping; return before `preventDefault()` unless the active line has an unambiguous embedded
    block transclusion.
- Risk: async target resolution cannot be completed before deciding whether to consume the keydown.
  - Mitigation: make the synchronous activation boundary the visible transcluded block-link context, and keep async
    source-task validation strict before writes.
- Risk: broadening status predicates could affect existing task toggles.
  - Mitigation: reuse the existing forced `"/"` path and `isNonTranscludedStartableStatus()` instead of changing
    `isOpenDoneTaskStatus()`.
- Risk: source/deployed plugin drift.
  - Mitigation: edit only the linked `bob-plugins` source, run the required sync command, and verify targeted diffs.
