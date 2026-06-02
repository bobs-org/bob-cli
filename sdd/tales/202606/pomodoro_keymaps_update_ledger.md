---
create_time: 2026-06-02 12:47:27
status: wip
prompt: sdd/prompts/202606/pomodoro_keymaps_update_ledger.md
---
# Pomodoro Keymaps Ledger Update Plan

## Goal

Fix the Obsidian Vim-mode `\p` and `\P` Pomodoro keymaps so they update both pieces of the Pomodoro ledger entry:

```markdown
- [ ] (**0900-0925** [t:: 25m]) Focus work
```

After this fix, `\p` should add one 5-minute unit to both the Dataview duration field and the visible ledger time range,
and `\P` should subtract one 5-minute unit from both. Repeat counts should continue to scale the unit count.

Examples:

```markdown
\p : - [ ] (**0900-0925** [t:: 25m]) Focus -> - [ ] (**0900-0930** [t:: 30m]) Focus

\P : - [ ] (**0900-0925** [t:: 25m]) Focus -> - [ ] (**0900-0920** [t:: 20m]) Focus

3\p : - [ ] (**0900-0925** [t:: 25m]) Focus -> - [ ] (**0900-0940** [t:: 40m]) Focus
```

This intentionally corrects the prior inline-`t` plan's behavior, which changed `\p` and `\P` to update only
`[t:: ...]`. The keymaps should keep their old visible-ledger duration editing behavior while also maintaining the new
Dataview duration field.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian plugin and ledger workflow context before planning keymap fix"`.
- Read `/home/bryan/bob/AGENTS.md`. The vault is actively synced; implementation must inspect status before edits,
  preserve unrelated dirty notes, and commit only task-related vault file changes before finishing.
- Current vault status has unrelated dirty note files, while
  `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js` is clean.
- Current plugin helper behavior was sampled with Node stubs. Today:
  `changePomodoroLineUnits("- [ ] (**0900-0925** [t:: 25m]) ...", 1)` produces `(**0900-0925** [t:: 30m])`, leaving the
  visible range unchanged.
- The parent of the inline-`t` vault commit showed the old `\p` / `\P` behavior: `changePomodoroUnits` rewrote the
  time-range end by `count * 5m`.
- Later bold-range changes made the canonical range shape `(**HHMM-HHMM** [t:: Nm])`; this fix should preserve that
  canonical shape and backward-compatible parsing.

## Product Decisions

1. Treat `\p` and `\P` as Pomodoro duration edits.
   - They should update `[t:: ...]`.
   - They should also update the ledger clock end so the visible range reflects the new duration.
   - They should not move the start time.

2. Keep `\o` and `\O` as range offset commands.
   - They should continue moving both start and end earlier/later.
   - They should preserve the current `[t:: ...]` metadata value.

3. Make the edited line internally consistent after `\p` / `\P`.
   - Compute the current duration from `[t:: ...]` when present and valid.
   - Fall back to legacy duration metadata if present.
   - Fall back to the parsed range duration when no duration metadata is available.
   - Compute the next duration as `max(0, currentDuration + repeatCount * 5m)`.
   - Rebuild the ledger range as `start -> start + nextDuration`, so the visible range and `[t:: ...]` agree after the
     keymap runs.
   - This means an already-mismatched line will be normalized on the next `\p` or `\P` edit. That is preferable to
     preserving a mismatch indefinitely.

4. Preserve migration and compatibility behavior.
   - Missing `[t:: ...]` should still be seeded from the range duration.
   - Legacy stopwatch metadata inside the parentheses should still be replaced with `[t:: Nm]`.
   - Existing compact and colon ranges should still parse.
   - Rewrites should continue canonicalizing to the bold range form.

## Implementation Scope

Expected file to change:

- `/home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js`

No Bob CLI change is expected. `bob pomodoro` already reads the ledger range and ignores `[t:: ...]` metadata; extending
or shortening the visible range through the plugin remains compatible with the current parser.

Implementation outline:

- Update `changePomodoroLineUnits(line, units)` so it no longer preserves `range.endMinutes`.
- Reuse existing helpers:
  - `parseTimeRange`
  - `pomodoroDurationMinutes`
  - `durationMetadata`
  - `formatTimeRange`
  - `addMinutes`
  - `numericOrDefault`
- Compute `unitDelta = numericOrDefault(units, 0) * STEP_MINUTES`.
- Compute `nextMinutes = max(0, currentMinutes + unitDelta)`.
- Compute `nextEndMinutes = addMinutes(range.startMinutes, nextMinutes)`.
- Rebuild the range with:
  - original start minutes;
  - computed end minutes;
  - original compact/colon style;
  - normalized duration metadata for `nextMinutes`.
- Leave `offsetPomodoroLineRange`, `replaceTimeRange`, and Vim mapping registration unchanged unless inspection during
  implementation finds a direct dependency that needs a small helper rename.
- Keep helper exports available for focused Node verification.

## Verification Plan

Run syntax checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-ledger-tools/manifest.json
```

Run focused Node helper assertions with stubbed `obsidian`, `@codemirror/state`, and `@codemirror/view` modules:

- `changePomodoroLineUnits("- [ ] (**0900-0925** [t:: 25m]) Focus", 1)` returns `- [ ] (**0900-0930** [t:: 30m]) Focus`.
- The same helper with `-1` returns `(**0900-0920** [t:: 20m])`.
- Repeat counts work, e.g. `3` units turns `25m` into `40m` and `0925` into `0940`.
- Missing `[t:: ...]` seeds from the parsed range and updates both the new field and the range end.
- Legacy stopwatch metadata still migrates to `[t:: ...]` and updates the range end.
- Decrements clamp at `0m` and set the visible end equal to the start time.
- Colon-style ranges preserve colon style while updating end and duration.
- Midnight wrapping still works through `addMinutes`.
- `offsetPomodoroLineRange` still moves start/end and preserves `[t:: ...]`.

Optional smoke check if implementation touches more than the helper:

```bash
cargo test pomodoro
```

Before finishing implementation:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If `/home/bryan/bob` files were changed, commit only the task-related vault file with the required `/sase_git_commit`
workflow, leaving the pre-existing dirty note files alone.

## Risks

- Normalizing mismatched `range` and `[t:: ...]` values changes the behavior for rare manually edited inconsistent
  lines. The plan accepts that because the keymaps are duration-edit commands and should produce a coherent ledger line.
- Clamping to zero needs to avoid wrapping the end time backwards. Deriving the end from `start + nextDuration` keeps
  the zero case as `start-start`.
- The vault has unrelated dirty synced notes. Implementation must stage or commit only
  `.obsidian/plugins/bob-ledger-tools/main.js`.
