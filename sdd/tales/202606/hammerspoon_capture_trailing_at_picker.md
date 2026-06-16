---
create_time: 2026-06-16 10:03:44
status: done
prompt: sdd/prompts/202606/hammerspoon_capture_trailing_at_picker.md
---
# Plan: Hammerspoon Capture Picker Only On Trailing Bare At

## Finding

The current Hammerspoon capture flow in the chezmoi source
`/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua` uses a staged picker flow:

1. Trim the submitted prompt text.
2. Run `bob capture --dry-run --format json -- "$1"` to learn whether Bob sees an explicit `@route`.
3. If routed, run the final capture.
4. If unrouted, run `bob capture-targets --format json` and show an `hs.chooser`.
5. The chooser includes `mac_inbox` first, and selecting that row runs the unrouted inbox capture path.

That shipped behavior intentionally made every plain unrouted task open the picker. Bryan now wants the old plain-text
behavior back:

- `foo bar baz` should capture immediately to `~/bob/mac_inbox.md`.
- `foo bar baz @cash` and `@cash foo bar baz` should continue to capture immediately using Bob's existing route parser.
- Only `foo bar baz @` should open the area/project picker.
- The trailing picker marker ` @` must not appear in the created task.
- The picker should no longer show `mac_inbox`, because plain Enter on the original prompt is now the inbox path.

The Rust CLI already treats a bare trailing `@` as literal text rather than a route token. This is a Hammerspoon UI
semantics change, not a Bob CLI contract change. The existing `bob capture-targets` JSON remains useful because
Hammerspoon can filter out inbox/default rows before building chooser choices.

## Goal

Update only the Hammerspoon capture integration so the area/project picker becomes an explicit opt-in suffix:

- Plain, unrouted input captures directly to inbox.
- Existing explicit `@route` prefix/suffix captures directly using Bob's own parser.
- A trailing bare marker, written as ` @` at the end of the prompt after trimming outer whitespace, opens the target
  picker.
- The final task is created from the text with that marker removed.
- The picker contains only area and project targets.

## Scope

Planned implementation file:

- `/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua`

Planned deployment action:

- Apply only the Hammerspoon target from chezmoi to `~/.hammerspoon/init.lua`.

Out of scope:

- No Bob CLI source changes.
- No change to `bob capture-targets` output ordering or schema.
- No vault-note, Obsidian, memory, or keybinding changes.
- No fallback that silently writes to inbox when the picker fails.

## Implementation Approach

1. Add a small Hammerspoon-side parser for the picker trigger.

   Reuse the existing `trimCaptureText` normalization at the UI boundary. After trimming outer whitespace, treat the
   input as a picker request only if it ends with a bare `@` preceded by whitespace. Practical matching should allow
   multiple spaces before the marker, but should not treat `foo@`, `foo @cash`, or `@cash foo` as picker requests.

   The helper should return:
   - the submitted text to capture;
   - whether the area/project picker was requested.

   For picker requests, strip the trailing whitespace-plus-`@` marker and trim again. If stripping leaves no task text,
   keep the current empty-input behavior: do not start a capture.

2. Simplify `submitCapturedTask`.

   The dry-run stage is no longer needed for normal submit behavior. Bob's final `capture` command already owns the
   authoritative route parser and inbox default.

   New submit flow:
   - If a capture task or chooser is already active, return.
   - Parse the UI marker.
   - If the resulting task text is empty, return.
   - If the marker was present, fetch targets and show the area/project chooser with the stripped task text.
   - Otherwise run `bob capture --format json -- "$1"` immediately with the trimmed original text.

   This restores `foo bar baz` to the old direct inbox behavior while preserving explicit `@route` behavior through the
   existing final-capture command.

3. Filter the chooser to area/project rows.

   Change the target-to-choice mapping so it includes only targets whose `kind` is `area` or `project`. This removes
   `mac_inbox` and any other inbox/default rows even if the CLI continues to emit them first.

   Preserve the remaining CLI order. Since `capture-targets` currently emits `mac_inbox` first, then areas, then active
   projects, filtering inbox rows leaves the intended area/project ordering intact.

4. Update chooser selection semantics.

   Since inbox is no longer present in the picker, every selected row should force its route via
   `bob capture --format json --route "$2" -- "$1"`.

   Dismissal should keep the existing behavior: close only the chooser, refocus the original prompt, and leave the typed
   prompt text intact. If the user typed `foo bar baz @` and presses Escape in the chooser, the prompt should still show
   `foo bar baz @`.

5. Keep failure handling explicit.

   Retain the existing `Task target picker failed` notification for `capture-targets` failures or an empty filtered
   choice list. Do not fall back to inbox from a failed picker. The user can remove the trailing marker and press Enter
   again when they want inbox capture.

6. Clean up now-unused dry-run plumbing.

   If `taskCaptureDryRunCommand` becomes unused after the submit-flow change, remove it and update comments that
   describe the old three-stage routed/unrouted detection flow.

## Verification

Local non-GUI checks:

```bash
luac -p /home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua
git -C /home/bryan/.local/share/chezmoi diff -- home/dot_hammerspoon/init.lua
chezmoi --source /home/bryan/.local/share/chezmoi apply ~/.hammerspoon/init.lua
cmp -s /home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua /home/bryan/.hammerspoon/init.lua
```

CLI contract smoke, using a scratch vault so no real notes are changed:

```bash
tmp="$(mktemp -d)"
BOB_DIR="$tmp" bob capture --format json -- "foo bar baz"
BOB_DIR="$tmp" bob capture --format json -- "foo routed @cash"
BOB_DIR="$tmp" bob capture --format json --route cash -- "foo bar baz"
```

Expected scratch-vault results:

- `foo bar baz` writes `mac_inbox.md`.
- `foo routed @cash` writes `cash.md` and strips `@cash` through Bob's normal parser.
- forced route writes `cash.md` while treating any remaining `@tokens` in the text literally.

Manual GUI smoke on macOS:

- Submit `foo bar baz`; confirm no picker appears and the task lands in `~/bob/mac_inbox.md`.
- Submit `foo bar baz @cash`; confirm no picker appears and the task lands in `~/bob/cash.md`.
- Submit `foo bar baz @`; confirm the area/project picker appears.
- Confirm `mac_inbox` is not present in the picker.
- Choose an area/project row and confirm the created task text is `foo bar baz`, not `foo bar baz @`.
- Submit `foo @cash @`, choose a different target, and confirm the selected target wins while `@cash` remains literal in
  the task body.
- Dismiss the picker with Escape and confirm the original prompt remains focused with the typed ` @` marker intact.

## Risks And Mitigations

- Ambiguous marker parsing: keep the trigger intentionally narrow, requiring a whitespace-separated bare trailing `@`.
- Regression in explicit routes: avoid reimplementing Bob's route parser; let final `bob capture` handle normal submits.
- Empty picker after filtering inbox: notify clearly and preserve the prompt instead of silently capturing elsewhere.
- Async stale callbacks: preserve the existing `taskCaptureTask` and `taskCaptureChooser` guards.
- Unrelated chezmoi work: inspect the chezmoi diff before and after, and apply only the Hammerspoon target.

## Success Criteria

- `foo bar baz` captures directly to inbox.
- `foo bar baz @` opens the area/project picker.
- Picker-created tasks do not include the trailing ` @` marker.
- `mac_inbox` is absent from the picker.
- Explicit `@route` captures still bypass the picker.
- Source and live Hammerspoon files match after apply.
