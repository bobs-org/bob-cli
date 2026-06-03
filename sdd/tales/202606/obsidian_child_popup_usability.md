---
status: planned
create_time: 2026-06-03 11:43:31
prompt: sdd/prompts/202606/obsidian_child_popup_usability.md
---

# Obsidian Child Popup Usability Plan

## Goal

Improve the existing Obsidian child-note popup so it is easier to scan and operate:

- Make the popup substantially wider and taller.
- Show more child notes at once.
- Show more of each child path/title without wrapping.
- Add `Ctrl+N` and `Ctrl+P` navigation inside the popup, while keeping the existing ArrowUp/ArrowDown and Enter
  behavior.

This is a focused follow-up to the already implemented child-note picker. No `bob-cli` Rust change is expected, and the
existing `Ctrl+Alt+C` command hotkey should remain unchanged.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault workflow context before planning child-note popup improvements"`.
- Read `/home/bryan/bob/AGENTS.md`; vault edits must preserve unrelated dirty files and, if implementation changes are
  made later, commit only task-related vault files with `/sase_git_commit`.
- Read the approved original plan at `sdd/tales/202606/obsidian_child_notes_popup.md`.
- Inspected the current plugin implementation in:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
  - `/home/bryan/bob/.obsidian/hotkeys.json`
- Current child picker facts:
  - `ChildNotePickerModal` is a plain Obsidian `Modal`.
  - The modal content currently gets class `bob-child-note-picker`, but the modal container itself is not tagged.
  - The plugin currently has no `styles.css`.
  - Results use Obsidian's built-in `suggestion-container`, `suggestion-item`, `suggestion-title`, and `suggestion-note`
    classes.
  - Keyboard handling currently supports ArrowDown, ArrowUp, and Enter from the focused filter input.
- Current vault status has unrelated dirty paths:
  - `obsidian.md`
  - `sase.md`
  - `2026/20260603_day.md`
  - `lib/`
  - `old_lib/`
  - `ref/`
- The task-related plugin files are currently clean at the last committed change:
  `5771fcc feat: add Obsidian child note picker`.

## Product Decisions

1. Keep this inside `bob-navigation-hotkeys`.
   - The popup already belongs to this plugin.
   - The requested behavior is a UI/keyboard refinement, not a new command or separate plugin.

2. Add plugin-scoped CSS instead of hard-coding layout styles in JavaScript.
   - Add `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`.
   - Use narrow, namespaced selectors under a modal-specific class so the styles affect only this picker.
   - Keep JavaScript responsible for behavior and class tagging, not detailed visual layout.

3. Tag the modal container as well as the content.
   - Keep `contentEl.addClass("bob-child-note-picker")`.
   - Also add a class such as `bob-child-note-picker-modal` to `this.modalEl` in `onOpen`.
   - This lets CSS size the actual Obsidian modal shell, not only the inner content.

4. Make the modal larger with responsive bounds.
   - Target a desktop width around `min(92vw, 1000px)` so long child paths get much more horizontal space.
   - Target a height around `min(82vh, 860px)` so more rows are visible.
   - Use a flex column layout so the title and filter stay fixed while the result list gets the available height.
   - Make the result list scroll internally instead of allowing the whole modal to grow past the viewport.
   - Add mobile-safe constraints so the popup still fits small screens.

5. Prevent result text from wrapping.
   - Apply `white-space: nowrap`, `overflow: hidden`, and `text-overflow: ellipsis` to child title/path rows.
   - Keep both basename and vault-relative path visible as separate lines, but each line should remain single-line.
   - Slightly tighten row padding/spacing if needed so the taller modal shows more children without making the list feel
     cramped.

6. Add `Ctrl+N`/`Ctrl+P` as list navigation aliases.
   - Extend `ChildNotePickerModal.handleKeydown(event)`.
   - Treat `Ctrl+N` as the same action as ArrowDown.
   - Treat `Ctrl+P` as the same action as ArrowUp.
   - Match keys case-insensitively, require `event.ctrlKey`, and avoid hijacking combinations with `Meta` or `Alt`.
   - Call `preventDefault()` and likely `stopPropagation()` for these modal navigation keys so the focused filter input
     and Obsidian/global commands do not consume them.
   - Preserve the current wrap-around selection behavior.

7. Leave unrelated behavior alone.
   - Do not change child collection, parent frontmatter resolution, sorting, filtering semantics, opening behavior,
     alternate-file tracking, Vim mappings, or persisted hotkeys.
   - Do not edit `/home/bryan/bob/.obsidian/hotkeys.json` for this request unless implementation reveals an unexpected
     need, which is not expected.

## Implementation Scope

Expected vault files to edit after plan approval:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`

Likely JavaScript changes:

- In `ChildNotePickerModal.onOpen()`:
  - add a class to `this.modalEl`, for example `bob-child-note-picker-modal`;
  - optionally add a class to `this.resultsEl` if existing `suggestion-container` selectors are too broad for clean CSS.
- In `ChildNotePickerModal.handleKeydown(event)`:
  - recognize ArrowDown or `Ctrl+N` as `moveSelection(1)`;
  - recognize ArrowUp or `Ctrl+P` as `moveSelection(-1)`;
  - keep Enter opening unchanged;
  - keep behavior safe when there are zero visible files.
- Optionally add a tiny helper such as `isControlKey(event, key)` or `isSelectionNavigationKey(event, key)` if that
  keeps the handler readable.

Likely CSS changes:

- Size only the child-note modal shell:
  - `.modal.bob-child-note-picker-modal { width: min(92vw, 1000px); max-width: 1000px; height: min(82vh, 860px); }`
- Make the inner content fill the shell:
  - `.bob-child-note-picker { height: 100%; display: flex; flex-direction: column; overflow: hidden; }`
- Make results consume available height and scroll:
  - `.bob-child-note-picker .suggestion-container { flex: 1 1 auto; overflow-y: auto; }`
- Prevent wrapping and preserve scannability:
  - `.bob-child-note-picker .suggestion-title, .bob-child-note-picker .suggestion-note { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }`
- Add a small-screen media query if needed to keep width/height within viewport.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
jq '.' /home/bryan/bob/.obsidian/hotkeys.json
git -C /home/bryan/bob diff --check -- .obsidian/plugins/bob-navigation-hotkeys/main.js .obsidian/plugins/bob-navigation-hotkeys/styles.css
```

Focused behavior checks with stubbed `obsidian` and `@codemirror/view` modules:

- ArrowDown still advances selection.
- ArrowUp still reverses selection.
- `Ctrl+N` advances selection exactly like ArrowDown.
- `Ctrl+P` reverses selection exactly like ArrowUp.
- `Ctrl+N`/`Ctrl+P` prevent default browser/input behavior.
- Enter still opens the currently selected child.
- Selection wrap-around still works.
- Zero-result filtering still does not throw and does not try to open a missing file.

Manual live-vault acceptance check:

- Open a note with several direct children.
- Press `Ctrl+Alt+C`.
- Confirm the popup is noticeably wider and taller than before.
- Confirm the child list scrolls inside the popup and shows more children at once.
- Confirm long child titles/paths stay on one line instead of wrapping.
- Use ArrowUp/ArrowDown and Enter to confirm existing keyboard behavior still works.
- Use `Ctrl+N` and `Ctrl+P` to move down/up through the list and Enter to open the selected child.
- Filter the list and confirm `Ctrl+N`/`Ctrl+P` work on the filtered results.

Before finishing implementation later:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If vault files are changed, commit only the task-related vault files with `/sase_git_commit`, leaving the unrelated
dirty vault paths alone.

## Risks

- Obsidian themes may style modal and suggestion classes differently. Namespacing the CSS under
  `bob-child-note-picker-modal` and `bob-child-note-picker` should keep the result stable without fighting theme styles
  globally.
- Very long paths can still exceed even a wider modal. Single-line text with ellipsis is the right fallback because it
  preserves row height and avoids wrapping.
- `Ctrl+N`/`Ctrl+P` may overlap with text-input movement conventions or app-level commands. Handling them while the
  modal's filter input is focused and stopping propagation should make list navigation win only inside this popup.
