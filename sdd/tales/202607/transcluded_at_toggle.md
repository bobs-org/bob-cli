---
create_time: 2026-07-07 19:27:00
status: done
prompt: sdd/prompts/202607/transcluded_at_toggle.md
---
# Plan: Toggle @ Transcluded Task Start State

## Context

The `task-status-cycler` plugin currently has a capture-phase `@` key handler in `plugins/task-status-cycler/main.js`.
In Vim normal mode, when the active editor line contains an unambiguous embedded block transclusion such as
`![[Note#^block-id]]`, the handler consumes the key and calls the transcluded-task start helper.

That helper is currently one-way:

- target `[ ]` resolves as startable and is forced to `[/]`
- target `[/]` does not resolve as startable, so the consumed key produces no source change
- target `[x]` and other non-open statuses stay untouched

The requested behavior is to keep the same keymap and targeting rules, but make the target task toggle between open and
in-progress:

- `[ ]` -> `[/]`
- `[/]` -> `[ ]`
- `[x]` and other statuses remain unchanged

## Implementation Plan

1. Keep the existing event boundary intact.

   The `@` listener should still only run for the exact unmodified `@` key, in an active Markdown editor, outside
   insert/visual/replace Vim modes, and only when the visible line has a supported embedded block transclusion target.
   Ordinary `@` macro playback and unrelated editor lines should continue to pass through.

2. Add a narrow start-toggle status helper.

   Add a helper for the `@` behavior that accepts only open and in-progress source task statuses. Do not broaden the
   existing `isNonTranscludedStartableStatus()` helper, because it is also used by Pomodoro non-transcluded start
   behavior where "start only open tasks" is still the right contract.

3. Compute the forced next symbol from the resolved target.

   Update the active transcluded-task helper so it resolves targets with the new open-or-in-progress predicate, then
   chooses the forced symbol from the resolved status:
   - resolved `" "` means force `"/"`
   - resolved `"/"` means force `" "`
   - anything else returns false/no-op

   Rename the helper from one-way "start" language to toggle language if that keeps the code clearer, and update the `@`
   dispatch call accordingly.

4. Tighten forced-write validation for forced open.

   The writer re-reads the target line before replacing it. Extend `canForceTranscludedTaskStatus()` with an explicit
   forced `" "` branch that only allows current `"/"` status. This prevents a stale async resolution from reopening a
   task that changed to done before the write. Existing unforced open/done toggles should remain unchanged.

5. Preserve transcluded-source rewrite semantics.

   Continue writing through the active editor when the target is in the active note, and through the vault for other
   notes. Continue using `rewriteTaskLineForTranscludedSource()` so Tasks-filtered lines keep the existing
   completion-field cleanup behavior when moving back to open.

6. Keep unrelated flows out of scope.

   Do not change recursive Pomodoro completion, Pomodoro child-link starting, regular open/done checkbox toggles,
   Obsidian task promotion/demotion, or bare/non-embedded block-link behavior.

## Verification Plan

1. Run static validation:

   ```bash
   node --check plugins/task-status-cycler/main.js
   npm run validate
   git diff --check -- plugins/task-status-cycler/main.js
   ```

2. Run focused mocked Node checks for the changed behavior:
   - open transcluded target rewrites `[ ]` to `[/]`
   - in-progress transcluded target rewrites `[/]` to `[ ]`
   - done target does not rewrite
   - forced open does not rewrite an already-open or done current line
   - ordinary `@` lines still do not call `preventDefault()`
   - visible embedded transclusion lines still consume the event and route to the toggle helper

3. Deploy the plugin source to the vault after source changes:

   ```bash
   bob plugins sync -p task-status-cycler -r <linked bob-plugins repo> -F
   ```

4. Verify deployment:
   - compare the deployed vault `task-status-cycler/main.js` against the linked repo source
   - run `bob plugins list -r <linked bob-plugins repo>` and confirm the plugin reports synced
   - check `git status --short --branch` in the linked repo
