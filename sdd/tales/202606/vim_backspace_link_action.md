---
create_time: 2026-06-05 15:15:35
status: planned
prompt: sdd/prompts/202606/vim_backspace_link_action.md
---

# Vim Backspace Previous-Line Link Action

## Goal

Add a Vim normal-mode Backspace keymap for the Bob Obsidian vault that mirrors the existing Enter link action, but
targets lines before the cursor instead of lines after it.

Target behavior:

- `<Backspace>` should behave like `1<Backspace>`: inspect the line immediately before the current line.
- `N<Backspace>` should inspect the line `N` lines before the current line.
- If the target line has one note-link candidate, open/create it immediately using the same candidate semantics as
  Enter.
- If the target line has multiple note-link candidates, open the same picker used by Enter.
- If the target line has no candidate, fall back to moving the cursor to the computed target line, clamped to the first
  editor line, matching Enter's current "move to target line" fallthrough in the opposite direction.

This is a line-offset action, not a "previous link anywhere above the cursor" search.

## Context Reviewed

- Read `memory/short/sase.md`.
- Read Obsidian long-term memory through `sase memory read long/obsidian.md`; the live vault is `~/bob`.
- Inspected the current live plugin state in:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`
  - `/home/bryan/bob/.obsidian.vimrc`
  - `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/main.js`
- Confirmed `bob-navigation-hotkeys/main.js` is already dirty from the approved same-file subpath link change, and
  `task-status-cycler/main.js` has no local diff.
- Confirmed the existing Enter keymap is owned by `task-status-cycler`: it maps normal-mode `<CR>`, delegates link
  handling to `bob-navigation-hotkeys.handleVimEnterLinkAction`, and falls through to line movement if no link action is
  handled.
- No `bob-cli` Rust CLI changes or new CLI subcommands are involved, so `memory/long/cli_rules.md` is not required.

## Product Decisions

1. **Backspace uses the same link candidate model as Enter.** All link types supported by `collectLineLinkCandidates`
   should work on the backward target line, including same-file subpath/block links from the just-completed Enter fix.

2. **Counts are line offsets.** `3<Backspace>` targets `cursor.line - 3`, just as `3<CR>` targets `cursor.line + 3`.
   Counts are normalized with the existing positive-integer Vim repeat helper.

3. **Clamp at buffer boundaries.** On the first line, Backspace targets the first line, matching Enter's existing
   clamp-at-last-line behavior. Larger counts above the top also clamp to the first line.

4. **Backspace does not toggle tasks.** The Enter keymap has task-status semantics because that is the current meaning
   of `<CR>`. Backspace should only run the backward link action, then fall back to backward line movement when no link
   is handled.

5. **Keep Vim mapping registration centralized.** Add the Backspace normal-mode mapping in `task-status-cycler`, next to
   the existing `<CR>` mapping, because that plugin already owns the CodeMirror Vim registration lifecycle and reliably
   delegates to the navigation plugin.

6. **Preserve the public Enter method.** Refactor `bob-navigation-hotkeys` around a direction-aware helper, but keep
   `handleVimEnterLinkAction(cm, actionArgs)` available so existing delegation and tests continue to work.

## Implementation Scope

Expected live-vault files to edit after this plan is approved:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`

No planned changes to `.obsidian/hotkeys.json`, `.obsidian.vimrc`, plugin manifests, styles, templates, or bob-cli
workspace source files.

### `bob-navigation-hotkeys/main.js`

- Add a direction-aware target-line helper, likely `getVimOffsetTargetLine(cm, actionArgs, direction)`, that:
  - reads the editor cursor,
  - normalizes repeat with `getVimRepeat`,
  - computes `cursor.line + direction * repeat`,
  - clamps with both first and last editor line helpers.
- Keep `getVimEnterTargetLine(cm, actionArgs)` as a wrapper with `direction = 1`.
- Add `getVimBackspaceTargetLine(cm, actionArgs)` or export the generic helper for tests, with `direction = -1`.
- Refactor `handleVimEnterLinkAction` into a shared line-link action helper:
  - `handleVimLineLinkAction(cm, actionArgs, direction)` computes the target line, reads that line, collects candidates,
    and opens the single candidate or picker exactly as the current Enter implementation does.
  - `handleVimEnterLinkAction` calls the shared helper with `direction = 1`.
  - `handleVimBackspaceLinkAction` calls the shared helper with `direction = -1`.
- Ensure the picker subtitle/target-line display still receives the actual target line number, now valid for either
  direction.
- Export new pure helpers under `module.exports.helpers` for the Node VM checks.

### `task-status-cycler/main.js`

- Define a new Vim action for Backspace, for example `taskStatusCyclerOpenPreviousLineLink`.
- Map normal-mode Backspace to that action using the CodeMirror Vim key token accepted by the live adapter. The likely
  token is `<BS>`; validate during implementation against the adapter/stub and use `<Backspace>` only if that is the
  accepted token in this Obsidian environment.
- Add `handleVimBackspaceLinkAction(cm, actionArgs)` that delegates to
  `bob-navigation-hotkeys.handleVimBackspaceLinkAction(cm, actionArgs)` and returns `true` only when the navigation
  plugin reports handled, mirroring the Enter delegation.
- Add a backward fallthrough helper or generalize `vimEnterFallthrough`:
  - Enter remains `cursor.line + repeat`, clamped to the last line.
  - Backspace becomes `cursor.line - repeat`, clamped to the first line.
  - Both fallthrough paths set the cursor to the first nonblank character of the target line, preserving current Enter
    behavior.
- Keep the existing task-toggle behavior for `<CR>` unchanged.

## Acceptance Criteria

- In Vim normal mode, with the cursor on a line below `[[SomeNote]]`, pressing `<Backspace>` opens `SomeNote`.
- With the cursor three lines below a link, `3<Backspace>` opens the link on that target line.
- If the target line has multiple links, Backspace opens the existing link candidate picker for that target line.
- If the target line has a same-file heading/block link, such as `[[#Heading]]` or `[[#^blockid]]`, Backspace jumps
  within the current note using the same behavior as Enter.
- If the backward target line has no note link, Backspace moves to that line's first nonblank character.
- At the top of the file, Backspace clamps to line 0 and does not throw or delete text.
- Existing Enter behavior is unchanged, including task-status toggling, forward line-link opening, candidate picker
  behavior, and Enter fallthrough movement.
- The mapping is normal-mode only and does not affect insert-mode Backspace.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
jq '.' /home/bryan/bob/.obsidian/plugins/task-status-cycler/manifest.json
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/task-status-cycler/main.js
```

Focused Node VM checks with stubbed `obsidian`, `@codemirror/view`, fake CodeMirror, and fake app/plugin registry:

- Target-line helper:
  - Enter default/repeat keeps current behavior (`line + 1`, `line + repeat`, clamp at last line).
  - Backspace default/repeat targets `line - 1`, `line - repeat`, clamp at first line.
  - Invalid/zero/negative/non-numeric repeats normalize to 1.
- Navigation action:
  - `handleVimBackspaceLinkAction` reads the previous line by default.
  - `handleVimBackspaceLinkAction` reads the counted previous line for `repeat > 1`.
  - Single candidate opens immediately.
  - Multiple candidates create a `LinkCandidatePickerModal` for the computed target line.
  - No candidates returns `false` without opening or creating.
  - `handleVimEnterLinkAction` still reads the counted following line.
- Task-status-cycler delegation:
  - Vim registration defines the Backspace action and maps it with `{ context: "normal" }`.
  - If the navigation plugin handles Backspace, no fallthrough cursor movement occurs.
  - If the navigation plugin is missing, returns false, or throws, Backspace falls through to backward line movement.
  - Backspace fallthrough clamps at the first line and sets the cursor to first nonblank.
  - Existing `<CR>` action still delegates/toggles/falls through as before.

Manual live-vault acceptance after implementation and plugin reload:

1. Create or use a scratch note with link lines, plain lines, a heading, and a block id.
2. Place the cursor one line below `[[SomeNote]]`; press Backspace; confirm `SomeNote` opens.
3. Place the cursor several lines below a link; press `N<Backspace>`; confirm the counted target line is used.
4. Use a target line containing `[[#Heading]]`, `[[#^blockid]]`, and `[label](#Heading)`; confirm Backspace jumps within
   the current note.
5. Use a target line with multiple links; confirm the picker appears and opens the selected target.
6. Use a target line with no links; confirm Backspace moves upward to first nonblank instead of deleting text.
7. Confirm normal Enter task toggling and forward link behavior still work.
8. Confirm insert-mode Backspace still deletes text normally.

Before finishing implementation:

```bash
git -C /home/bryan/bob status --short -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/task-status-cycler/main.js \
  .obsidian/hotkeys.json \
  .obsidian.vimrc
git status --short
```

## Risks

- **Backspace key token mismatch.** CodeMirror Vim often names Backspace as `<BS>`, while Obsidian/vimrc-support also
  recognizes `Backspace` as a DOM key. Mitigation: validate the mapping token during implementation and cover the final
  token in a registration test.
- **Cross-plugin load order.** `task-status-cycler` may load before `bob-navigation-hotkeys`; this is already handled by
  delegation returning false for Enter. Backspace should use the same resilient lookup and fallthrough.
- **Behavior at file boundaries.** Clamping can make Backspace on line 0 inspect line 0. This intentionally mirrors
  Enter on the last line and avoids unhandled key behavior.
- **Refactor regression in Enter.** Sharing direction-aware code could alter Enter behavior. Mitigation: keep the public
  Enter wrapper, add forward-direction regression checks, and manually verify task toggling.
- **Dirty vault state.** The vault has many unrelated dirty files. Implementation should touch only the two plugin files
  above and leave `.obsidian/hotkeys.json`, `.obsidian.vimrc`, and unrelated notes untouched.
