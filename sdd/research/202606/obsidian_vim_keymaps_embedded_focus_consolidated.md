---
create_time: 2026-06-15
status: research
topic: Workarounds for Obsidian Vim normal-mode keymaps losing focus on embedded UI such as transcluded Bases tables
---
# Research: Obsidian Vim Keymaps Over Embedded UI Focus

## Question

Bob's Obsidian vault uses Vim mode and `obsidian-vimrc-support` normal-mode maps such as
`-`, `[[`, `]]`, `!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, and `\|`. These mappings
stop working when the focused thing is not the Markdown editor, with a transcluded Bases
table as the main example. Is there a workaround, what are the alternatives, and what should
we use?

## Short Answer

This is a focus-boundary problem, not a broken individual mapping. Obsidian's Vim maps run
inside the CodeMirror Markdown editor. A transcluded Bases table is a separate interactive
widget, so when it owns DOM focus, `codemirror-vim` does not receive the keydown events and
the `nmap`s cannot fire.

The best workaround is to stop treating these maps as Vim-only. In this vault, the Vim maps
already call real Obsidian commands from `bob-navigation-hotkeys` and `bob-ledger-tools`.
That gives us a better dispatch surface: use native Obsidian hotkeys or a small custom
keydown router to invoke the same commands when focus is outside the editor.

## Current Vault State

Verified locally from `/home/bryan/bob`:

- `.obsidian/app.json` has `"vimMode": true` and `"showLineNumber": true`.
- Relevant enabled plugins include `obsidian-vimrc-support`, `bob-navigation-hotkeys`,
  `bob-ledger-tools`, `task-status-cycler`, `dataview`, `metadata-menu`, and
  `mrj-jump-to-link`.
- Vimrc Support reads `obsidian_vimrc.md`; `supportJsCommands` is `false`.
- The vimrc maps are thin bridges. For example, `nmap [[ :bob_prev_link<CR>` calls an
  `exmap` that runs `obcommand bob-navigation-hotkeys:open-prev-link`; `nmap -` calls
  `bob-ledger-tools:open-today-daily-note`; `nmap !` calls
  `bob-navigation-hotkeys:toggle-line-transclusions`.
- `bob-navigation-hotkeys/main.js` registers the target actions with `this.addCommand(...)`.
  Several are plain `callback` commands, while editor-specific operations such as toggling
  line transclusions, inserting blank lines, and jumping section headers use
  `editorCallback`.
- `task-status-cycler` already uses CodeMirror Vim mappings plus a capture-phase keydown
  listener for one physical hotkey. Its listener is still editor-targeted, so it is not the
  complete solution here, but it is useful local precedent for carefully guarded DOM-level
  key capture.

## Key Findings

### 1. Vimrc Support cannot see keys once CodeMirror loses focus

CodeMirror Vim bindings are enabled on a CodeMirror editor by setting the editor keymap to
Vim. Obsidian Vim mode and Vimrc Support build on that editor-scoped mechanism. The Vimrc
Support README also frames mappings as editor normal-mode behavior: commands are tested by
typing `:` in Obsidian's normal mode.

That means a selected or focused embed is outside the layer where `nmap` is handled.
Enabling Vimrc Support JavaScript commands would not fix this; the plugin still needs the
editor's Vim key dispatcher to receive the keystroke first. Keeping `supportJsCommands:
false` is not the cause of this issue.

### 2. Bases has its own keyboard model, not the editor's Vim model

Obsidian 1.10.3 public added substantial Bases table keyboard support: table selection,
full keyboard navigation, copy/paste, undo/redo, and table hotkeys such as Enter, Tab,
Shift-Tab, Home, End, PageUp/PageDown, row/column selection, and clearing cells.

That is useful for table work, but it does not make `obsidian_vimrc.md` maps fire while a
Base owns focus. It means Bases now has native keyboard behavior of its own.

### 3. Upstream fixes are targeted, not a general custom-map bridge

Obsidian 1.13.0 includes fixes around embedded inputs and selected elements, including
`Ctrl/Cmd-A` inside embedded inputs such as Bases cells and better image selection behavior
that supports Vim for built-in image operations. This confirms the class of issue is real,
but it also shows the likely shape of upstream work: specific built-in keys and specific
widgets get patched over time. It should not be expected to make arbitrary user `nmap`s
work over every embed.

### 4. The hard design constraint is bare keys vs. text input

Vim can safely map bare keys such as `-`, `!`, `[[`, and `]]` because they fire only in
normal mode. App-level hotkeys do not have that protection. A global bare `-` would break
typing unless the handler is gated so it only runs when focus is outside editable fields.

The workaround therefore has to choose one of three compromises:

- use modifier chords such as `Alt+J`, `Alt+K`, or `Alt+-`;
- use a modifier leader key plus sequences;
- preserve the bare keys only with custom focus/editability checks.

Also, even native Obsidian hotkeys can be intercepted while actively editing an embedded
input. That is a feature, not a bug, for this use case: we do not want `-` or `!` to run Bob
commands while typing in a Bases cell.

## Alternative Approaches

### A. Use native Bases keyboard navigation for table-internal work

For selecting cells, moving through rows/columns, editing Base data, copying cells, and
clearing selections, use the native Bases table hotkeys. Verify Obsidian is at least 1.10.3
public, and preferably test on the current desktop build because embedded-input fixes have
continued after that.

Pros: native, maintained by Obsidian, and aware of Bases semantics.
Cons: does not restore Bob's custom Vim maps; it only reduces the need for them while inside
the table.

### B. Manually refocus the Markdown editor

Press Escape if the widget supports it, click back into the note body, or bind a native
hotkey to a focus-editor command if a reliable command is available.

Pros: no code and no new plugin.
Cons: this is the friction we are trying to remove; it is a recovery move, not a workflow.

### C. Bind native Obsidian hotkeys to the same commands

Because the target actions are already Obsidian commands, bind modifier chords in Settings
-> Hotkeys for the commands you need over embeds. Start with:

- `bob-navigation-hotkeys:jump-to-next-section-header`
- `bob-navigation-hotkeys:jump-to-prev-section-header`
- `bob-navigation-hotkeys:open-prev-link`
- `bob-navigation-hotkeys:open-next-link`
- `bob-ledger-tools:open-today-daily-note`
- `bob-navigation-hotkeys:open-dash-tasks`

Pros: no code, no new dependency, and works through Obsidian's command/hotkey layer instead
of CodeMirror Vim.
Cons: it uses a second set of chords, cannot preserve bare-key muscle memory, and must be
tested over specific embeds because focused inputs may consume some key events.

### D. Use a leader-key plugin

Install a plugin such as Spacekeys or Leader Hotkeys, bind one app-level modifier leader
such as `Ctrl+M`, and map mnemonic sequences to the same `bob-*` commands.

Pros: one global entry point can reach many commands, and it avoids assigning many modifier
chords.
Cons: more keystrokes than the Vim maps, another plugin dependency, and the same
actively-editing-input caveat as native hotkeys.

### E. Add a guarded bare-key router to `bob-navigation-hotkeys`

Add a small capture-phase `keydown` handler owned by `bob-navigation-hotkeys`. It would run
only when:

- the active view is a Markdown view;
- the event target is not inside `.cm-editor`;
- `document.activeElement` / `event.target` is not `input`, `textarea`, `select`, or
  `[contenteditable=true]`;
- the event is not composing text, not already prevented, and not using unrelated modifiers;
- the key sequence matches one of the Bob maps.

Then dispatch the existing commands. Plain callback commands can usually go through
`app.commands.executeCommandById(...)`. For `editorCallback` commands, test whether Obsidian
supplies the active Markdown editor while an embed has focus. If not, call the plugin method
directly after resolving `app.workspace.getActiveViewOfType(MarkdownView).editor`, or first
refocus the active editor before invoking the command.

This router needs a short sequence buffer for multi-key maps such as `[[`, `]]`,
`[<Space>`, and `]<Space>`. Keep the timeout short and handle only the exact existing maps.
Do not try to emulate all of Vim outside CodeMirror.

Pros: preserves the exact bare-key muscle memory over selected embeds, leaves normal
CodeMirror Vim behavior untouched, avoids a new dependency, and can be tested narrowly
against the embed types we use.
Cons: custom code to maintain, with careful safety checks needed so it never hijacks text
entry inside Bases cells or other editable UI.

### F. Wait for upstream or avoid interactive embeds

Obsidian will likely continue to improve individual keyboard cases. Alternatively, avoid
interactive transclusions where they interrupt editing.

Pros: no implementation work.
Cons: unlikely to solve arbitrary custom `nmap`s, and avoiding interactive embeds gives up
the feature that caused the issue.

## Recommended Solution

Use a two-step approach.

First, use native Bases keyboard navigation for real Bases work, and add a few native
modifier hotkeys for the Bob commands that need to work when a table or other embed is
selected. This is the fastest low-risk workaround and validates which commands matter most
outside CodeMirror.

Then implement a guarded bare-key router in `bob-navigation-hotkeys` for only those
high-value Bob maps. That is the best long-term fit because the actions are already
first-class commands we own, and it is the only option that preserves the current Vim muscle
memory over selected non-editor widgets without trying to make Vimrc Support own UI it
cannot see. The router should explicitly stand down inside CodeMirror and all editable
fields, and it should be tested against a transcluded Bases table in both selected-cell and
editing-cell states.

Net: keep Vimrc Support for editor-normal-mode mappings, use native Bases keys inside
Bases, and move cross-focus Bob command dispatch into `bob-navigation-hotkeys`.

## Sources

- [Obsidian 1.10.3 Desktop changelog - Bases keyboard navigation and table hotkeys](https://obsidian.md/changelog/2025-11-11-desktop-v1.10.3/)
- [Obsidian 1.13.0 Desktop changelog - embedded input and selected image keyboard fixes](https://obsidian.md/changelog/2026-05-28-desktop-v1.13.0/)
- [Obsidian Help: Introduction to Bases](https://obsidian.md/help/bases)
- [obsidian-vimrc-support README](https://github.com/esm7/obsidian-vimrc-support/blob/master/README.md)
- [CodeMirror Vim bindings documentation](https://codemirror.net/5/demo/vim.html)
- [Spacekeys](https://github.com/jlumpe/obsidian-spacekeys)
- [Leader Hotkeys for Obsidian](https://github.com/tgrosinger/leader-hotkeys-obsidian)
- Local vault files inspected: `/home/bryan/bob/obsidian_vimrc.md`,
  `/home/bryan/bob/.obsidian/app.json`,
  `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`,
  `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`, and
  `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js`.
