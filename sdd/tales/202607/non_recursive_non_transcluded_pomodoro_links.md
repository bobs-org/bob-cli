---
create_time: 2026-07-07 18:19:22
status: wip
prompt: sdd/prompts/202607/non_recursive_non_transcluded_pomodoro_links.md
---
# Plan: Make Bare Pomodoro Task Link Starts Non-Recursive

## Goal

Update the Obsidian `task-status-cycler` Pomodoro behavior so bare non-transcluded block links in Pomodoro sub-bullets
only start the directly linked Obsidian task. The plugin should stop scanning the linked task's own descendant
sub-bullets for additional bare non-transcluded links.

Embedded transcluded links remain different: `![[...#^id]]` Pomodoro behavior should still recursively complete embedded
target trees to done. This change is only for plain non-embedded links such as `[[project#^task-id]]`.

The implementation source of truth is the linked `bob-plugins` repo, in `plugins/task-status-cycler/main.js`. Do not
edit deployed vault plugin files under `~/bob/.obsidian/plugins/` directly; deploy source changes with
`bob plugins sync -p task-status-cycler` from the linked source repo after implementation.

## Context Reviewed

- Read Obsidian long-term memory with `sase memory read obsidian.md` because this changes Obsidian vault/plugin task
  behavior.
- Opened the linked `bob-plugins` repo with `sase workspace open -p bob-plugins`.
- Inspected the current committed implementation in `plugins/task-status-cycler/main.js`, including:
  - strict bare non-embedded block-link detection;
  - Pomodoro sub-bullet classification;
  - full Pomodoro completion wiring;
  - direct Pomodoro sub-bullet `<Ctrl+Enter>` handling;
  - non-transcluded in-progress traversal helpers;
  - forced `[/]` rewrite guards.
- Reviewed the original approved plan for marking bare non-transcluded Pomodoro links in progress. This follow-up keeps
  the root-link start behavior from that plan but intentionally reverses the recursive descendant behavior.
- No `bob-cli` subcommands or options are being added, so `memory/cli_rules.md` is not required.

## Current State

The current plugin now has two separate Pomodoro source-task paths:

- Embedded transcluded sub-bullets (`![[note#^id]]`) are completed recursively to `[x]`.
- Strict bare non-transcluded sub-bullets (`[[note#^id]]`) are started recursively to `[/]`.

The non-transcluded recursion is isolated around these pieces:

- `classifyPomodoroSubBullets()` keeps broad `copyableTaskLinkBullets` for copy-forward behavior and narrow
  `bareNonTranscludedTaskLinkBullets` for the in-progress side effect.
- `startPomodoroNonTranscludedTaskBullets()` processes the local Pomodoro's narrow bare-link roots.
- `startNonTranscludedTaskTargetTree()` and `startResolvedNonTranscludedTaskTargetTree()` resolve a root target, scan
  the resolved task's descendant list-item block, and recursively follow any strict bare child links they find.
- `collectBareNonTranscludedTaskTargetsInListItemBlock()` exists only to support that recursive child scan.
- `startActivePomodoroNonTranscludedTaskLine()` uses the same recursive resolved-target helper for direct `<Ctrl+Enter>`
  on an eligible bare sub-bullet.

The status-write policy is already right for the requested non-recursive behavior:

- open proper `#task` targets become `[/]`;
- already `[/]` or `[x]` targets are not rewritten;
- done tasks are not reopened to `[/]`;
- blocked, canceled, custom status, non-task, and non-`#task` blocks are skipped.

## Desired Behavior

Completing an open Pomodoro should still scan the local Pomodoro sub-bullets and start each eligible strict bare
non-transcluded root link. It should not inspect any sub-bullets on the resolved source task.

Example:

```markdown
## Pomodoros

- [ ] #task Work session
  - [[Project#^parent]]
```

```markdown
- [ ] #task Parent task ^parent
  - [[Next#^child]]
```

Completing the Pomodoro should change only `^parent` to `[/]`. It should leave `^child` unchanged.

The same non-recursive rule should apply to direct `<Ctrl+Enter>` on an eligible bare non-transcluded Pomodoro
sub-bullet: start only the selected root target, without completing the local Pomodoro and without following child links
from the source task.

Existing local Pomodoro semantics should remain unchanged:

- broad non-embedded task-link bullets are still copied forward as they are today;
- strict bare non-embedded link detection still controls only the in-progress side effect;
- embedded transcluded bullets still complete recursively and are not copied forward;
- note bullets, placeholder creation, deduping, cursor placement, and centering behavior stay unchanged.

## Product Decisions

1. Keep the local Pomodoro scan.
   - "Not recursive" means stop after each directly linked root target.
   - It does not mean only the first local Pomodoro sub-bullet should be processed.
   - If a Pomodoro has several eligible strict bare root links, each root is still considered independently.

2. Treat resolved non-transcluded targets as leaves.
   - Do not call any helper that scans the resolved task's descendant list-item block.
   - Do not follow `[[#^child]]` or `[[other#^child]]` links found under the resolved task.
   - Cycle/depth/target caps are no longer relevant to non-transcluded starts because there is no graph traversal.

3. Preserve root target status semantics.
   - `[ ]` proper `#task` root targets become `[/]`.
   - `[/]` and `[x]` proper `#task` root targets resolve successfully but stay unchanged.
   - `[-]`, `[B]`, arbitrary custom statuses, non-`#task` checkboxes, non-task blocks, unresolved links, and malformed
     targets are skipped without aborting sibling roots.

4. Preserve direct-key handling semantics.
   - Direct `<Ctrl+Enter>` on a strict bare non-transcluded link under an open Pomodoro should be considered handled
     once the root resolves as an eligible proper task, even if no write is needed because it is already `[/]` or `[x]`.
   - This avoids falling through to unrelated embedded-link fallback behavior.
   - The open-parent-Pomodoro guard should remain.

5. Keep embedded transclusion recursion untouched.
   - Recursive forced-done behavior for `![[...#^id]]` is still useful and intentionally separate.
   - Same-file descendant rebasing, seen sets, depth caps, and target caps remain part of the embedded transclusion
     path.

## Implementation Approach

1. Replace the non-transcluded "tree" operation with a leaf operation.
   - Refactor `startNonTranscludedTaskTargetTree()` into a non-recursive helper such as
     `startNonTranscludedTaskTarget()`.
   - Refactor `startResolvedNonTranscludedTaskTargetTree()` into a non-recursive helper such as
     `startResolvedNonTranscludedTaskTarget()`.
   - Keep target resolution through the existing block-link resolver so root file/path/block-id behavior stays the same.
   - Keep the proper `#task` line predicate and the existing forced `[/]` write guard.

2. Keep root dedupe, remove descendant traversal.
   - `startPomodoroNonTranscludedTaskBullets()` can keep a shared seen set keyed by resolved `file.path#^block-id` so
     duplicate local root links do not attempt redundant writes.
   - Remove the loop that calls `collectBareNonTranscludedTaskTargetsInListItemBlock()` on the resolved source text.
   - Remove non-transcluded use of recursion depth and target caps.

3. Delete recursion-only bare-child collection code.
   - Remove `collectBareNonTranscludedTaskTargetsInListItemBlock()` if it has no remaining callers.
   - Remove its helper export.
   - Keep `getBareNonEmbeddedBlockLinkTargetFromListItem()` because it is still needed for local Pomodoro classification
     and direct-line detection.

4. Update comments and names that imply traversal.
   - Adjust comments around non-transcluded starts so they describe root-only behavior.
   - Adjust the top recursion-guard comment so it is clearly about embedded transcluded completion.
   - Rename traversal-oriented predicate wording where touched, or at minimum update comments so future work does not
     reintroduce descendant scanning by following stale names.

5. Preserve the full Pomodoro completion order.
   - Keep embedded forced-done processing before bare non-transcluded root starts.
   - Keep bare non-transcluded root starts before the local Pomodoro completion plan is built and applied.
   - Reread editor lines before building the local Pomodoro plan, preserving the current same-file safety pattern.

6. Preserve direct sub-bullet handling order.
   - In `handleVimTaskToggleOpenDone()`, embedded Pomodoro transclusions should still win first.
   - Strict bare non-transcluded Pomodoro sub-bullets should still be handled next.
   - Generic embedded transcluded toggle fallback should remain last.

7. Deploy from source after implementation.
   - Run `bob plugins sync -p task-status-cycler` from the linked `bob-plugins` source repo.
   - Verify the deployed vault plugin copy matches the source file byte-for-byte.

## Acceptance Criteria

- Completing an open Pomodoro whose sub-bullet is exactly `- [[project#^a]]`, where `^a` is `- [ ] #task ... ^a`,
  changes only the source task `^a` to `- [/] #task ... ^a`.
- If the resolved `^a` task contains descendant strict bare links such as `- [[project#^b]]`, `^b` is not resolved,
  inspected, or rewritten.
- The non-embedded sub-bullet is still copied forward to the next/new Pomodoro exactly as it is today.
- A sub-bullet with extra text, such as `- work on [[project#^a]]`, is still eligible for existing copy-forward behavior
  but does not mark any linked task in-progress.
- A line with multiple non-embedded block links is copied forward as today but does not trigger in-progress marking.
- Alias links such as `- [[project#^a|Task A]]` are eligible when they are the only bullet body content.
- Already in-progress and done root targets are not rewritten, and their descendants are not scanned.
- Done tasks are never reopened to `[/]`.
- Canceled, blocked, arbitrary custom statuses, non-`#task` checkbox blocks, non-task blocks, unresolved links, and
  malformed root targets are skipped without aborting the rest of the Pomodoro completion.
- Same-file root links such as `[[#^a]]` still resolve relative to the active note.
- Same-file descendant links inside a resolved source task, such as `[[#^child]]`, are ignored for this non-transcluded
  start behavior.
- Pressing `<Ctrl+Enter>` directly on a strict bare non-transcluded sub-bullet under an open Pomodoro starts only the
  selected root target without completing the Pomodoro, creating/copying a placeholder, or following child links.
- Pressing `<Ctrl+Enter>` on generic non-embedded block links outside an open Pomodoro sub-bullet keeps current
  behavior.
- Existing embedded transcluded forced-done recursion remains unchanged.
- Existing local `[ ]`/`[x]` toggles remain unchanged.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/task-status-cycler/main.js
```

Focused Node checks with helper exports and a stubbed Obsidian app:

- Strict bare-link detector still accepts `- [[note#^id]]`, `- [[folder/note#^id|Alias]]`, and `- [[#^id]]`.
- Strict bare-link detector still rejects embedded links, note links, heading links, malformed block IDs, multiple
  links, and links with surrounding explanatory text.
- Pomodoro sub-bullet classification still preserves broad `copyableTaskLinkBullets` while adding only strict bare links
  to the root-start collection.
- Non-transcluded root-start status checks classify `[ ]`, `[/]`, `[x]`, `[-]`, `[B]`, and custom statuses as expected.
- Non-transcluded root-start processing changes an open proper `#task` root target to `[/]`.
- Non-transcluded root-start processing leaves `[/]` and `[x]` root targets unchanged.
- Non-transcluded root-start processing skips non-`#task` checkboxes and non-task block targets.
- Duplicate local root links perform at most one write for the same resolved target.
- A resolved root task containing strict bare child links does not cause any child target resolution or write.
- A resolved root task containing same-file child links such as `[[#^child]]` leaves the child unchanged.
- Full Pomodoro completion still closes embedded transcluded trees, starts strict bare non-embedded root targets only,
  copies non-embedded bullets forward, and applies the existing cursor/placeholder behavior.
- Direct strict bare sub-bullet `<Ctrl+Enter>` handles only open-Pomodoro sub-bullet contexts and starts only the root
  target.

Manual smoke test after `bob plugins sync -p task-status-cycler` and Obsidian plugin reload:

1. Create scratch daily-style and source notes with `#task` block-ID tasks.
2. Complete a Pomodoro with a strict bare non-embedded link to an open source task; confirm the source task becomes
   `[/]` and the bullet is copied forward.
3. Repeat with a source task that has child strict bare non-embedded links; confirm only the linked root task becomes
   `[/]` and child linked tasks remain unchanged.
4. Repeat with already `[/]` and `[x]` root tasks containing open child links; confirm roots stay unchanged and child
   linked tasks remain unchanged.
5. Confirm decorated or multi-link bullets are copied forward but do not rewrite source tasks.
6. Press `<Ctrl+Enter>` directly on a strict bare non-embedded sub-bullet under an open Pomodoro and confirm only the
   linked root task is marked in-progress.
7. Confirm embedded transcluded Pomodoro links still close linked trees to `[x]` recursively.

## Risks and Mitigations

- Risk: interpreting "not recursive" as disabling all but one local Pomodoro sub-bullet. Mitigation: keep local Pomodoro
  sub-bullet scanning and process every eligible root; stop only after resolving each root target.
- Risk: stale traversal-oriented helper names could make future maintenance reintroduce child scanning. Mitigation:
  remove recursion-only helper code and update comments/names where the non-transcluded path is touched.
- Risk: direct `<Ctrl+Enter>` on an already `[/]` or `[x]` root could fall through to unrelated fallback behavior.
  Mitigation: preserve the current "handled once the root resolves" contract.
- Risk: removing non-transcluded recursion might accidentally affect embedded recursive completion because both paths
  share target resolution and write helpers. Mitigation: keep embedded `completeTranscludedTaskTargetTree()` untouched
  and add focused checks proving embedded recursion still works.
- Risk: source and deployed vault plugin copies drift. Mitigation: deploy with `bob plugins sync -p task-status-cycler`
  from the linked source repo and compare deployed plugin output to source after sync.
