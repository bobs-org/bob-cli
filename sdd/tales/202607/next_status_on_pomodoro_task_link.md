---
create_time: 2026-07-08 13:04:34
status: done
prompt: sdd/prompts/202607/next_status_on_pomodoro_task_link.md
---
# Plan: Set a pomodoro-linked task to the new `[*]` "Next" status when completing a `^^` task link

## Summary

Extend the existing `^^` task-picker feature (in the `block-id-prompt` Obsidian plugin, source-of-truth in the
`bob-plugins` linked repo) so that, when the user completes a `^^` block link **and the link is a sub-bullet of a
pomodoro entry**, the selected target task is also flipped to the new **Next** status (`[*]`) — but **only if the target
task is currently open (`[ ]`)**, never if it is in-progress (`[/]`) or already `[*]`.

This is purely additive plugin behavior. No new status is defined (the `[*]` Next status already exists after the prior
"Blocked → Next" migration), no CLI change, and no styling change.

## Background — the `^^` task-picker flow today

When the user types a trailing `^^` (or `#^^`) inside a wiki link, e.g. `[[some_note^^]]`, the plugin's editor scan
(`inspectActiveEditor` → `findTaskPickerMarkerNearCursor`) recognizes a `link-task-picker` marker and opens the
**`TaskLinkPickerModal`**. That modal lists the **open tasks** in the target note (`collectTaskPickerItems` →
`getOpenTasksInContent`, which surfaces `#task` lines whose checkbox status is one of
`OPEN_OBSIDIAN_TASK_STATUSES = [" ", "/", "*"]`).

When the user picks a task, `TaskLinkPickerModal.openTaskAtIndex` calls `plugin.selectTaskLinkTask(source, task)`, and
the flow splits into **two completion paths**:

1. **Target task already has a block id** (`task.existingId` set) → `completeTaskLinkWithExistingId(source, task)`. This
   validates the target line is unchanged, then calls `completeTaskSourceLink(source, task.existingId)` to rewrite the
   source `^^` marker into a finished `[[target#^id]]` link. **The target note is not modified in this path.**

2. **Target task has no block id yet** (`task.existingId` falsy) → `selectTaskLinkTask` returns a `promptSource`
   (`kind: "link-task-complete"`), which opens the `BlockIdPromptModal`. On submit,
   `submitLinkTaskBlockId(source, newId)` runs: `appendTaskBlockId(...)` writes ` ^<newId>` onto the **target task
   line**, then `completeTaskSourceLink(source, newId)` finishes the source link.

Key facts established during investigation:

- The `source` object carries `source.editor` (the live CodeMirror editor of the note where `^^` was typed),
  `source.sourcePath`, and `source.line` (0-indexed cursor line). This is enough to inspect the surrounding lines and
  decide whether the source line is a pomodoro sub-bullet.
- Each picker `task` carries `task.status` (the raw checkbox char), `task.line`, and `task.rawLine`. Both completion
  paths re-validate that `task.rawLine` is still present in the target before writing, so `task.status` is trustworthy
  at write time.
- The picker only ever surfaces open tasks (` `, `/`, `*`), so we never touch done/cancelled tasks.
- Target writes already handle "target note is the same file as the source" (via `source.editor.replaceRange`) vs. "a
  different note" (via `app.vault.modify`), using a read-snapshot + `content !== expectedContent` guard
  (`readFileSnapshot`, `appendTaskBlockId`). Any new target write must reuse this exact mechanism.

### What a pomodoro sub-bullet looks like (from `~/bob/` daily notes)

Under a `## Pomodoros …` heading, each pomodoro is an **indent‑0 checkbox ledger line** whose content is a time range or
an empty placeholder:

```
## Pomodoros (…)

- [x] (**1100-1150** [t:: 50m])
	- Improve [[sase]] `?` panel!
	- [[#^gtd]]
- [ ] ()
	- [[bob#^wip-tasks-query]]
		- some deeper note!
```

The **sub-bullets** (where a `^^` link is typed) are tab-indented list items beneath the pomodoro entry: `- [[target]]`,
`- ![[target]]` (embed), or free text, occasionally nested a second level deep. The Pomodoros section runs from the
`## Pomodoros` heading to the next `## ` heading (or end of file). This exact structure is already parsed canonically by
the **`bob-ledger-tools`** plugin (`POMODOROS_HEADING_RE`, `LEVEL_TWO_HEADING_RE`, `LEDGER_LINE_RE`, `PLACEHOLDER_RE`,
`parseTimeRange`, `findPomodorosSection`, `parseLedgerLine`).

## Goal / Requirements

When a `^^` task link is completed:

- **Iff** the completed link line is contained in a pomodoro sub-bullet (a descendant of a pomodoro entry inside the
  `## Pomodoros` section of the source note), change the selected target task's checkbox to `[*]` (Next).
- Make this change **only if the target task is currently open** (`[ ]`). Do **not** change it if it is in-progress
  (`[/]`); leave `[*]` tasks untouched (already Next → no-op / no write).
- Apply in **both** completion paths — the existing-block-id path and the newly-assigned-block-id path ("given a block
  ID, if necessary").
- If the link is not a pomodoro sub-bullet, behavior is exactly as today (no status change).

## Design

All changes are confined to the `bob-plugins` repo file `plugins/block-id-prompt/main.js` (plus deploy to the vault via
`bob plugins sync`). `block-id-prompt` is an independent Obsidian bundle and cannot import from `bob-ledger-tools`, so
the small amount of pomodoro-recognition logic is **replicated** in `block-id-prompt`, with a source-of-truth comment
pointing at `bob-ledger-tools` so the two stay in sync.

### 1. Detect "the source line is a pomodoro sub-bullet"

Add pure helpers (module scope) mirroring `bob-ledger-tools`:

- Constants: `POMODOROS_HEADING_RE = /^##\s+Pomodoros(?:\s.*)?$/`, `LEVEL_TWO_HEADING_RE = /^##\s+/`, a ledger-line test
  (`LEDGER_LINE_RE = /^(\s*(?:[-*+]|\d+[.)])\s+\[([ /xX-])\]\s+)/`), `PLACEHOLDER_RE = /\(\s*\)/`, and the time-range
  detection (reuse `bob-ledger-tools`' colon/compact time-range regexes, or a faithful subset). Note the source of
  truth.
- `findPomodorosSectionRange(lines)` → `{ startLine, endLine }` or `null` (copy of `findPomodorosSection`).
- `isPomodoroEntryLine(lineText)` → true when the line is a checkbox ledger line **and** contains a `()` placeholder or
  a `(**HHMM-HHMM** …)` time range (i.e., it is a pomodoro entry, not an ordinary sub-bullet).
- `lineIndentWidth(lineText)` → length of the leading `[ \t]*` run (tabs counted as single chars; the vault indents
  pomodoro sub-bullets with tabs, consistently).
- `isPomodoroSubBulletLine(lines, lineNumber)`:
  1. `section = findPomodorosSectionRange(lines)`; return false if `null` or `lineNumber` is outside
     `[section.startLine, section.endLine]`.
  2. Let `indent = lineIndentWidth(lines[lineNumber])`.
  3. Walk upward from `lineNumber - 1` to `section.startLine`, skipping blank lines. Find the **nearest ancestor** — the
     first list line whose indent is strictly less than `indent`. Return `true` iff that ancestor
     `isPomodoroEntryLine(...)`; otherwise `false`.
  - This correctly rejects the pomodoro entry line itself (indent‑0, no shallower ancestor), free-text/link sub-bullets
    that live outside the section, and (defensively) any indented line whose real parent is not a pomodoro entry. It
    accepts sub-bullets nested one or more levels under an entry.

Wrap this in an instance method, e.g. `sourceLineIsPomodoroSubBullet(source)`, that reads
`source.editor.getValue().split("\n")` and calls `isPomodoroSubBulletLine(lines, source.line)`. (Using the live editor
buffer keeps it correct even for the same-file / unsaved case.)

### 2. Decide when to flip

Add a helper `shouldPromoteTaskToNext(source, task)` → boolean:

```
task.status === " "  &&  sourceLineIsPomodoroSubBullet(source)
```

`task.status === " "` enforces "open only": `/` (in-progress) and `*` (already Next) are excluded, matching the
requirement and avoiding a redundant write. (Done/cancelled can't reach here — the picker never lists them.)

### 3. Apply the checkbox flip in both paths (shared write helper)

Add a pure edit helper `setCheckboxStatusEdit(content, lineNumber, newStatus)` that produces an
`{ start, end, replacement }` edit rewriting just the checkbox character of the task line (matching the list-marker +
`[x]` prefix, e.g. via a `TASK_CHECKBOX_RE = /^(\s*(?:>\s*)*(?:[-+*]|\d+[.)])\s+\[)[^\]\n](\])/`), and factor the
existing target-write tail of `appendTaskBlockId` (editor vs. `vault.modify`, with the `readFileSnapshot` +
`content !== expectedContent` guard and `taskLineStillPresent` re-check) into a reusable
`applyTaskLineEdit(file, source, edit, expectedContent)`.

- **Path 2 (new block id) — `submitLinkTaskBlockId` / `appendTaskBlockId`:** when
  `shouldPromoteTaskToNext(source, task)`, the target line is rewritten **once** to both flip the checkbox to `*` and
  append ` ^<newId>` (a single combined line edit — build the new line as
  `setCheckboxStatus(task.rawLine, "*") + " ^" + newId`). When not promoting, keep today's append-only edit unchanged.
  Doing it as one edit avoids a second read/validate round-trip on the same line.

- **Path 1 (existing block id) — `completeTaskLinkWithExistingId`:** the target note is not modified today. When
  `shouldPromoteTaskToNext(source, task)`, add a standalone target write (via `applyTaskLineEdit`) that flips the
  checkbox to `*`, performed **before** `completeTaskSourceLink` (mirroring Path 2's target-then-source ordering). When
  not promoting, the path is unchanged (source-link write only).

- Both paths keep every existing pre-write guard (`sourceMarkerStillPresent`, `readDestinationForValidation`,
  `taskLineStillPresent`, block-id validation, snapshot equality). The status flip is layered on top; if any guard
  fails, we bail exactly as today with no partial status change.

### 4. User feedback (minor)

Optionally extend the existing success `Notice`s so the user knows the status moved, e.g. append " · set Next" when a
flip occurred ("Linked task · set Next" / "Added block ID and linked task · set Next"). Cosmetic; can be dropped if not
desired.

## Edge cases & decisions

- **Same-file links** (e.g. `[[#^gtd]]` pointing at a task in the same daily note): handled — the target write uses the
  live editor when `file.path === source.sourcePath`. The source-link edit is on a different line than the target task
  line, and neither edit changes line counts, so line/offset positions stay valid (same invariant the current
  block-id-append already relies on).
- **In-progress target** (e.g. today's `- [/] #task [[gtd_daily]] … ^gtd`, referenced from many pom sub-bullets): the
  `task.status === " "` guard skips it. Correct per requirement.
- **Already `[*]` target**: skipped (no write), idempotent.
- **Non-pomodoro `^^` links** (task picker used elsewhere, e.g. in a project note): `isPomodoroSubBulletLine` returns
  false → today's behavior, no status change.
- **`^^` typed directly on a pomodoro entry line** (indent‑0): not a sub-bullet (no shallower pom-entry ancestor) → no
  flip.
- **Partial-write window (Path 1):** if the source marker changes between selection and the synchronous
  `completeTaskSourceLink`, the status could flip without the link completing. This window is tiny (existing-id
  completion has no intervening modal) and mirrors Path 2's pre-existing "block ID added, but source link changed"
  partial state. Acceptable; emit an analogous Notice.
- **Cross-plugin drift:** the replicated pomodoro regexes must be kept faithful to `bob-ledger-tools`. Add a comment
  naming it as the source of truth. (A future refactor to a shared module is out of scope.)

## Out of scope (deliberately not changed)

- Defining/renaming any status, CSS/styling, or the Tasks-plugin config — the `[*]` Next status already exists.
- The `task-status-cycler`, pomodoro navigation, and ledger flows — untouched.
- The bob-cli Rust code, docs, or fixtures — this is a plugin-only behavior change.
- Changing which tasks the picker lists, or the dependency-based `is not blocked` filtering.
- Promoting tasks linked from _non_-pomodoro contexts, or handling in-progress tasks.

## Verification

There is no JS unit-test harness in `bob-plugins` (only `npm run validate` = `scripts/validate-manifests.mjs`).
Verification is manifest-validate + manual in-vault:

1. `npm run validate` in the `bob-plugins` repo passes.
2. Grep the edited `plugins/block-id-prompt/main.js` to confirm the new helpers exist and the two completion paths both
   consult `shouldPromoteTaskToNext`.
3. Deploy with `bob plugins sync` and reload the plugin in Obsidian, then manually confirm:
   - In a daily note's `## Pomodoros` section, add a pom entry and a sub-bullet; type `[[<note-with-open-task>^^]]`,
     pick an **open** (`[ ]`) `#task`. The link completes **and** that task's checkbox becomes `[*]` (a block id is
     added first if it lacked one). It picks up the Next styling and appears under the dashboard **NEXT Tasks** section.
   - Repeat targeting an **in-progress** (`[/]`) task → link completes, status stays `[/]`.
   - Repeat where the target task is in the **same** daily note → link + status flip both apply correctly.
   - Do the same `^^` completion **outside** a pomodoro sub-bullet (e.g. a normal project note) → link completes, target
     status unchanged.
   - Confirm nothing else regresses: `@`-rename, caret-completion, and inline block-link flows still behave.

## Rollout / operational notes

- Edit only the **source** in the `bob-plugins` linked repo (`plugins/block-id-prompt/main.js`); the deployed copy under
  the vault's `.obsidian/plugins/` is overwritten on sync.
- Deploy from the linked-repo checkout: `bob plugins sync -p block-id-prompt -r "$PWD" --dry-run` then without
  `--dry-run` (the default source path does not exist in SASE workspaces, so `-r "$PWD"` is required). Reload the plugin
  in Obsidian afterward to load the new `main.js`.
- Committing the `bob-plugins` change is left to the normal per-repo commit flow and is not performed as part of
  implementation unless requested.
