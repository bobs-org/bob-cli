---
create_time: 2026-07-07 19:39:53
status: done
prompt: sdd/prompts/202607/counted_transclusion_keymaps.md
---
# Plan: Counted `!` and `@` Transcluded Task Keymaps

## Goal

Make the Bob Obsidian Vim-normal `!` and `@` keymaps support explicit numeric counts for bulk work over adjacent task
block link lines.

The count semantics are defined by the example in the prompt: an explicit count `N` means "the current line plus `N`
following source lines." So `2!` on a task block link line attempts to operate on three lines total: the cursor line,
the next line, and the line after that. A bare `!` or bare `@` keeps today's one-line behavior.

## Context Reviewed

- Read Obsidian long-term memory through `sase memory read obsidian.md`.
- Opened the linked `bob-plugins` source repo as required; plugin source changes should happen there, then be deployed
  with `bob plugins sync`.
- Inspected `plugins/bob-navigation-hotkeys/main.js`.
  - It owns `toggle-line-transclusions`.
  - The current command toggles links only on the active line.
  - The live `obsidian_vimrc.md` maps `!` to this command through `obcommand`.
- Inspected `plugins/task-status-cycler/main.js`.
  - It owns the current `@` behavior.
  - `@` is intentionally implemented as a guarded capture-phase keydown listener, not a Vim mapping, so ordinary Vim
    macro playback remains available outside visible embedded block-transclusion lines.
  - The current task helper toggles transcluded source tasks between open `[ ]` and in-progress `[/]`.
- Reviewed prior SDD notes for `@`, the original `!` vimrc migration, and the repeat-count bugfix.
- Confirmed from the local Obsidian bundle and prior SDD notes that CodeMirror Vim action mappings receive
  `{ repeat, repeatIsExplicit }`, but `obcommand` does not forward repeat/count data into Obsidian commands.
- No `bob-cli` subcommands or options are being added, so `memory/cli_rules.md` is not required.

## Product Decisions

1. Keep bare behavior unchanged.
   - Bare `!` continues to toggle transclusions on only the current line through the existing command path.
   - Bare `@` continues to toggle only the active line's transcluded source task.
   - Existing notices and silent no-op behavior should stay stable for uncounted use.

2. Interpret an explicit count as extra following lines.
   - `1!` and `1@` operate on the current line plus one following line.
   - `2!` and `2@` operate on the current line plus two following lines.
   - The range clamps at end-of-file.

3. Count ranges are physical source-line ranges, not "find the next N eligible task links."
   - Lines without eligible content inside the range are skipped.
   - The active line must still be eligible before the counted key consumes the event.
   - This keeps count behavior predictable and avoids jumping over unrelated text.

4. Preserve `@` macro playback.
   - Do not add a global `vim.mapCommand("@", ...)`.
   - Continue using a capture-phase physical key listener.
   - Only consume `@` when the active line has an unambiguous embedded block transclusion candidate.
   - Otherwise return before `preventDefault()` so Vim macros still work.

5. Add count support for `!` without relying on `obcommand`.
   - Since `obcommand` drops repeat data, add a narrow capture-phase listener in `bob-navigation-hotkeys` that handles
     only explicit counted `!` presses.
   - Leave the existing `obsidian_vimrc.md` bare `!` mapping in place for no-count behavior.
   - If the counted handler does not activate, fall through to the existing Vim mapping.

6. Clear CodeMirror Vim's pending numeric prefix after consuming a counted physical key.
   - For physical key listeners, the count lives in `cm.state.vim.inputState` before CodeMirror sees the final key.
   - Read it with `inputState.getRepeat()`.
   - After consuming the event, reset the pending Vim input state using the current input state's constructor when
     available, with a defensive fallback that clears repeat/key-buffer/operator fields.
   - This prevents a handled `2!` or `2@` from leaving `2` pending for the next Vim command.

## Implementation Approach

### 1. Shared Vim-count Helpers

Add small local helpers in each affected plugin, or duplicate a minimal version if keeping plugin boundaries clean is
simpler:

- `getPendingVimRepeat(cm)`:
  - returns `{ repeat, explicit }`;
  - `explicit` is true when `cm.state.vim.inputState.getRepeat()` returns a positive integer;
  - invalid, missing, zero, and negative values normalize to no explicit count.
- `resetPendingVimInputState(cm, reason)`:
  - resets `cm.state.vim.inputState` after a physical handler consumes the counted key;
  - is best-effort and never turns a successful key action into a failure.

Keep these helpers focused on physical capture listeners. Do not change the existing `getVimRepeat(actionArgs)` helpers
used by real Vim action mappings.

### 2. Counted `!` in `bob-navigation-hotkeys`

Add a capture-phase counted-transclusion listener modeled on existing physical-key listeners:

- Register on `window` and `document`, with a `WeakSet` to avoid duplicate handling.
- Match literal `event.key === "!"`, no Ctrl/Alt/Meta modifiers, active Markdown editor, and Vim normal mode.
- Require an explicit pending count; bare `!` should fall through to the existing vimrc mapping.
- Require the active line to have at least one transclusion-toggle target before consuming.
- Build the source-line range `[cursor.line, cursor.line + repeat]`, clamped to the document.

Extend the pure transclusion-toggle logic to operate over multiple lines:

- Reuse `findTransclusionToggleTargets()` for each line.
- Determine the bulk mode across all targets in the range:
  - if every target in the range is already transcluded, remove `!` markers from all targets;
  - otherwise add `!` markers to every non-transcluded target.
- Replace only lines that actually change.
- Keep the cursor on the original line and adjust its column using the active line's changes, matching current
  single-line behavior.

This preserves the existing "toggle all recognized links on the line" behavior, while making multi-line ranges feel like
one bulk toggle instead of inverting mixed lines independently.

### 3. Counted `@` in `task-status-cycler`

Extend the current guarded `@` listener:

- Read the pending explicit count from the resolved normal-mode Vim CodeMirror object.
- If no explicit count exists, preserve the current one-line path.
- If an explicit count exists, build the range `[activeLine, activeLine + repeat]`, clamped to the document.
- Consume the event only after confirming the active line has the same unambiguous embedded block transclusion candidate
  required today.
- Reset pending Vim input state after consuming.

Add line-target collection helpers:

- Refactor `getActiveLineTranscludedTaskTarget()` around a lower-level helper that accepts an explicit line number and
  optional cursor column.
- For the active line, keep today's cursor-based disambiguation.
- For following lines in the counted range, accept a line only when it has exactly one embedded block transclusion, or
  when the existing unambiguous-candidate helper can identify exactly one candidate without relying on the active
  cursor.
- Skip non-transcluded links, non-block embeds, ambiguous multi-target lines, and non-task targets.

Apply task-state changes sequentially:

- For each collected target, call the same resolver/writer path used by bare `@`.
- Keep per-target toggle semantics: `[ ] -> [/]`, `[/] -> [ ]`, all other statuses skipped.
- Deduplicate by resolved source file path plus block id before writing, so duplicated visible links in the counted
  range do not toggle the same source task twice.
- Process sequentially rather than concurrently to avoid racing vault writes when multiple targets live in the same
  source file.

### 4. Deployment

Edit only the linked `bob-plugins` source files:

- `plugins/bob-navigation-hotkeys/main.js`
- `plugins/task-status-cycler/main.js`

Then deploy from source:

```bash
bob plugins sync -p bob-navigation-hotkeys -p task-status-cycler
```

Do not edit deployed plugin copies directly under the vault. Do not edit `obsidian_vimrc.md` unless implementation
proves the counted physical `!` listener cannot coexist with the existing bare mapping.

## Acceptance Criteria

- With the cursor on a block-link line followed by two block-link lines, `2!` toggles transclusion syntax on all three
  lines.
- Bare `!` still toggles only the current line.
- Counted `!` uses one bulk mode across the range:
  - all selected links embedded -> all become non-embedded;
  - otherwise all selected links become embedded.
- The cursor remains on the original line after counted `!`, with column adjusted for marker insertion/removal on that
  line.
- With the cursor on a transcluded task block link line followed by two similar lines, `2@` toggles the source task
  start state for all three eligible targets.
- Bare `@` still toggles only the active visible transcluded task target.
- `@` still does not consume ordinary macro playback on lines without an eligible embedded block transclusion.
- A counted handled `!` or `@` does not leave the numeric prefix pending for the next Vim command.
- Existing `<Ctrl+Enter>`, `<Enter>`, Backspace, open-task jumps, and vimrc command mappings remain unchanged.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/bob-navigation-hotkeys/main.js
node --check plugins/task-status-cycler/main.js
git diff --check -- plugins/bob-navigation-hotkeys/main.js plugins/task-status-cycler/main.js
```

Focused Node/helper checks where practical:

- Pending Vim repeat helper:
  - no input state -> not explicit;
  - `getRepeat() === 0` -> not explicit;
  - `getRepeat() === 2` -> explicit repeat `2`;
  - reset helper replaces or clears pending repeat state without throwing.
- Bulk `!` helper:
  - two bare block links plus one embedded block link become all embedded;
  - all embedded links become all bare;
  - no-link lines in the range are skipped;
  - active-line cursor adjustment matches the existing single-line helper.
- Bulk `@` target collection:
  - active line uses cursor disambiguation;
  - following lines with one embedded block link are collected;
  - following ambiguous lines are skipped;
  - duplicate targets are deduplicated.
- `@` event dispatch:
  - no eligible active-line target does not call `preventDefault()`;
  - eligible counted target consumes the event and resets pending Vim input.

Manual smoke after `bob plugins sync` and plugin reload:

1. In a scratch note, create three adjacent bare task block link lines and press `2!`; confirm all three become
   embedded.
2. Press `2!` again on the same group; confirm all three become bare.
3. With three adjacent embedded task block link lines pointing to open source tasks, press `2@`; confirm all three
   source tasks become in-progress.
4. Press `2@` again; confirm those in-progress source tasks return to open.
5. On an ordinary line, run a known Vim macro with `@<register>` and with a count such as `2@<register>`; confirm macro
   playback still works.
6. After a handled `2!` or `2@`, press a simple motion such as `j`; confirm it moves once, not twice.

## Risks and Mitigations

- Risk: physical key handlers rely on CodeMirror Vim internals for pending counts.
  - Mitigation: keep helpers tiny, defensive, and covered by stubs; use the known `inputState.getRepeat()` path from the
    local Obsidian bundle.
- Risk: consuming counted keys without clearing input state could corrupt the next Vim command.
  - Mitigation: reset the pending Vim input state immediately after a counted handler consumes an event, and manually
    smoke-test the next command.
- Risk: `@` macro playback regresses.
  - Mitigation: keep the existing active-line eligibility gate before `preventDefault()`, and do not introduce a global
    `@` Vim mapping.
- Risk: bulk `@` writes race when multiple targets are in the same source file.
  - Mitigation: resolve and write sequentially, with duplicate target suppression.
- Risk: source/deployed plugin drift.
  - Mitigation: edit linked plugin source only, run `bob plugins sync`, and verify targeted diffs after deployment.
