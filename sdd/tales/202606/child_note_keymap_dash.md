---
status: planned
create_time: 2026-06-03 13:03:44
prompt: sdd/prompts/202606/child_note_keymap_dash.md
---

# Child Note Picker Ctrl-Dash Keymap Plan

## Goal

Change the Obsidian child-note picker command from `Ctrl+Alt+C` to `Ctrl+-` (`control` + dash/minus).

This is a persisted hotkey configuration change, not a plugin behavior change. The command id remains
`bob-navigation-hotkeys:open-child-note`, and the popup implementation should keep all existing collection, filtering,
selection, and opening behavior.

## Context Reviewed

- Read project short memory from `memory/short/sase.md`.
- Read Obsidian long memory through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault and hotkey context before planning child note picker keymap change"`.
- Read the prior child-note popup implementation/redesign plans:
  - `sdd/tales/202606/obsidian_child_notes_popup.md`
  - `sdd/tales/202606/obsidian_child_popup_redesign.md`
- Inspected the live vault command registration: `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- Inspected the live persisted hotkeys: `/home/bryan/bob/.obsidian/hotkeys.json`
- Current command registration already has:
  - plugin id: `bob-navigation-hotkeys`
  - command id: `open-child-note`
  - full hotkey key: `bob-navigation-hotkeys:open-child-note`
  - callback: `() => this.openChildNotePicker()`
- The current persisted hotkey is:

```json
"bob-navigation-hotkeys:open-child-note": [
  {
    "modifiers": [
      "Ctrl",
      "Alt"
    ],
    "key": "C"
  }
]
```

- `/home/bryan/bob/.obsidian/hotkeys.json` is already dirty only because its trailing newline is missing. The actual
  hotkey content currently matches `HEAD`. The implementation should preserve unrelated content and avoid using a broad
  formatter that rewrites the whole file unnecessarily.

## Implementation Scope

Expected file to edit:

- `/home/bryan/bob/.obsidian/hotkeys.json`

Expected change:

```json
"bob-navigation-hotkeys:open-child-note": [
  {
    "modifiers": [
      "Ctrl"
    ],
    "key": "-"
  }
]
```

No expected edits to:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
- Any `bob-cli` Rust or script files

## Verification

Static checks:

```bash
jq '.' /home/bryan/bob/.obsidian/hotkeys.json
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
git -C /home/bryan/bob diff --check -- .obsidian/hotkeys.json
git -C /home/bryan/bob diff -- .obsidian/hotkeys.json
```

The final diff should show only the `open-child-note` hotkey changing from `Ctrl+Alt+C` to `Ctrl+-`, plus restoration of
the trailing newline if needed.

Manual live-vault acceptance:

- Open a Markdown note with direct child notes.
- Press `Ctrl+-` and confirm the child-note picker opens.
- Press the old `Ctrl+Alt+C` and confirm it no longer opens the picker, unless Obsidian still has another user-level
  binding outside this JSON.
- Confirm existing picker navigation still works once open: ArrowUp/ArrowDown, `Ctrl+N`/`Ctrl+P`, Enter, click, and
  Escape.

Before finishing:

```bash
git -C /home/bryan/bob status --short
git status --short
```

If committing is requested later, commit only the task-related vault file via the required SASE commit workflow and
leave unrelated vault changes untouched.

## Risks

- `Ctrl+-` may overlap with a built-in Obsidian command or an OS/window-manager shortcut, especially zoom-out semantics.
  The JSON change is still the requested mapping; manual live-vault testing should confirm whether Obsidian receives the
  chord in this environment.
- Obsidian stores the dash/minus key as `"-"` in `hotkeys.json`. If live testing shows Obsidian normalizes it
  differently, update only that single `key` value to Obsidian's serialized representation.
- Because `hotkeys.json` is already dirty from a newline-only difference, the implementation must inspect the diff
  before and after editing so it does not accidentally hide unrelated hotkey changes.
