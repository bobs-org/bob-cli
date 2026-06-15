---
create_time: 2026-06-15 15:49:29
status: wip
prompt: sdd/prompts/202606/obsidian_embed_keymap_fallback.md
---
# Plan: Obsidian Vim Keymap Fallback Over Focused Embeds (e.g. transcluded Bases tables)

## Context

Bryan's Obsidian vault (`~/bob`) runs with Vim mode (`.obsidian/app.json` → `"vimMode": true`) and uses
`obsidian-vimrc-support` to map bare Vim normal-mode keys to Bob commands in `~/bob/obsidian_vimrc.md`:

| Vim map    | exmap target               | Plugin command                                       | Command kind     |
| ---------- | -------------------------- | ---------------------------------------------------- | ---------------- |
| `-`        | `bob_daily`                | `bob-ledger-tools:open-today-daily-note`             | `callback`       |
| `[[`       | `bob_prev_link`            | `bob-navigation-hotkeys:open-prev-link`              | `callback`       |
| `]]`       | `bob_next_link`            | `bob-navigation-hotkeys:open-next-link`              | `callback`       |
| `\|`       | `bob_dash_tasks`           | `bob-navigation-hotkeys:open-dash-tasks`             | `callback`       |
| `!`        | `bob_toggle_transclusions` | `bob-navigation-hotkeys:toggle-line-transclusions`   | `editorCallback` |
| `[<Space>` | `bob_blank_line_above`     | `bob-navigation-hotkeys:insert-blank-line-above`     | `editorCallback` |
| `]<Space>` | `bob_blank_line_below`     | `bob-navigation-hotkeys:insert-blank-line-below`     | `editorCallback` |
| `<C-j>`    | `bob_next_header`          | `bob-navigation-hotkeys:jump-to-next-section-header` | `editorCallback` |
| `<C-k>`    | `bob_prev_header`          | `bob-navigation-hotkeys:jump-to-prev-section-header` | `editorCallback` |

**The problem.** These mappings run inside the CodeMirror Markdown editor. When an interactive embed — the motivating
case is a transcluded Bases table — owns DOM focus, keydown events never reach CodeMirror's Vim dispatcher, so none of
the `nmap`s fire. The user is effectively "trapped": they cannot navigate away (`[[`, `]]`, `-`, `\|`) or collapse the
embed (`!`) with their normal keys. This is a focus-boundary problem, not a broken individual mapping. (Full analysis:
[sdd/research/202606/obsidian_vim_keymaps_embedded_focus_consolidated.md](sdd/research/202606/obsidian_vim_keymaps_embedded_focus_consolidated.md).)

**Why not the simpler options.** Enabling Vimrc Support JS commands does not help — the plugin still needs CodeMirror to
receive the keystroke first. Native Obsidian hotkeys (Settings → Hotkeys) are a valid fallback but force a second set of
modifier chords (losing bare-key muscle memory) and, for the `editorCallback` commands, may not fire reliably when no
editor owns focus. The research's recommended long-term fix — the one this plan implements — is a small **guarded
capture-phase keydown router owned by `bob-navigation-hotkeys`** that re-dispatches the existing Bob commands when, and
only when, focus is on a non-editable embed inside the active Markdown note.

**Verified facts gathered from the vault (the research only read the compiled `main.js`; these were re-confirmed against
the actual code for this plan):**

- The router target file is `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (a directly-edited JS plugin, not
  compiled from TS). Pure helpers are exported via `module.exports.helpers` and exercised by ad-hoc Node tests with a
  mocked `obsidian` module — the established validation pattern for this plugin.
- The `callback` commands (`open-prev-link`, `open-next-link`, `open-dash-tasks`, and ledger-tools'
  `open-today-daily-note`) take no editor and run unconditionally, so `app.commands.executeCommandById(id)` fires them
  regardless of which element has focus. The daily-note command was confirmed to be a plain `callback`.
- The `editorCallback` commands are thin wrappers over plugin methods the router can call directly on a resolved host
  editor: `toggleCurrentLineTransclusions(editor)`, `insertBlankLine(editor, "above"|"below")`, and
  `jumpToSectionHeader(editor, direction)`. Routing these through `executeCommandById` is unreliable precisely because
  no editor owns focus over an embed — so the router calls the methods directly instead.
- `getActiveMarkdownView()` already exists and returns the host `MarkdownView` (with `.editor`) for the focused note.
- Precedent for a capture-phase keydown listener already lives in the sibling `task-status-cycler` plugin: it registers
  `keydown` (capture phase) on `window` + `document`, dedupes events with a `WeakSet`, and cleans up via
  `this.register`. Its guard is the **inverse** of what we need — it requires the target to be inside `.cm-editor`; our
  router fires only when the target is **outside** `.cm-editor`.
- The vault is actively synced by Obsidian Sync and may carry unrelated uncommitted changes; per `~/bob/AGENTS.md`, only
  the files changed for this task may be staged/committed, via `/sase_git_commit`.

## Goal

Make the Bob Vim keymaps work when an interactive embed (transcluded Bases table, and embeds generally) owns focus,
without changing their behavior in the normal editor, without a new plugin dependency, and without hijacking text entry
inside an embed's editable cells.

## Behavior Specification

The router activates on a keydown only when **all** of these hold (otherwise it stands down and lets the event pass):

1. There is an active `MarkdownView` in edit mode (source or live preview). Reading mode is out of scope — consistent
   with the existing vimrc maps and prior plugin plans.
2. The event target is inside that view's `containerEl` (so focus in sidebars, the file explorer, modals, the command
   palette, etc. is ignored).
3. The event target is **not** inside `.cm-editor` (when it is, CodeMirror Vim already handles the key — stand down).
4. The event target is **not** an editable field: not an `input`/`textarea`/`select` and not `isContentEditable`. This
   is what prevents the router from stealing keys while a Bases cell is in edit mode — by design, `-`/`!`/`\|` should
   _not_ run Bob commands while typing into a cell.
5. The event is not composing IME text and not already `defaultPrevented`.
6. Modifiers match the map: bare keys (`-`, `!`, `\|`, `[`, `]`, Space) require no Ctrl/Alt/Meta; the `<C-j>`/`<C-k>`
   chords require Ctrl only. Key identity is read from `event.key` so `!` and `\|` stay keyboard-layout independent.

When the guard passes and a key (or buffered multi-key sequence) matches a Bob map, the router calls
`preventDefault()` + `stopPropagation()` and dispatches:

- **Navigation / open commands** — `-`, `[[`, `]]`, `\|`: dispatched via `app.commands.executeCommandById(...)`. These
  are cursor-independent and are the primary, highest-confidence win — they let the user escape a focused embed.
- **Cursor-moving commands** — `<C-j>`, `<C-k>`: call `jumpToSectionHeader(view.editor, ±1)` directly. Non-destructive
  (they only move + scroll the cursor).
- **Editor-mutating commands** — `!`, `[<Space>`, `]<Space>`: see Phasing. These act relative to the embed the user is
  focused on, not a possibly-stale host cursor.

**Multi-key sequences** (`[[`, `]]`, `[<Space>`, `]<Space>`) use a short-lived prefix buffer: a bracket prefix is held
for ~600 ms awaiting its second key. Because the router only ever acts when focus is on a non-editable embed,
`preventDefault`-ing the buffered prefix key is harmless. On timeout or a non-matching second key, the buffer clears and
nothing is dispatched. The router never attempts to emulate Vim generally — only this exact, fixed set of maps.

## Phasing

**Phase 1 — router + cursor-independent + non-destructive maps.** Build the capture-phase router and guard, the prefix
buffer, and dispatch for `-`, `[[`, `]]`, `\|` (via `executeCommandById`) plus `<C-j>`/`<C-k>` (direct
`jumpToSectionHeader` on the host editor). This alone solves the core complaint — being unable to navigate away from a
focused embed — at high confidence and with no document mutation.

**Phase 2 — embed-aware editor mutations.** Add `!`, `[<Space>`, `]<Space>`. To make these intuitive over an embed (and
to avoid mutating at a wrong/stale cursor), resolve the **embed's own source line** from the focused DOM element via the
CodeMirror `EditorView.posAtDOM(embedEl)` → `state.doc.lineAt(pos)` path (walking up to the embed block element), set
the editor cursor to that line, then invoke the existing method. `!` then toggles the transclusion the user is looking
at; `[<Space>`/`]<Space>` insert a blank line directly above/below the embed. If source-line resolution fails (API
unavailable or throws), the router **stands down** for these mutating commands and shows a brief Notice rather than
editing the wrong line.

## Implementation Sketch

All changes are in `~/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`:

1. **Pure, testable matcher** (top-level function, exported in `module.exports.helpers`): given the current
   prefix-buffer state and a normalized key descriptor, return one of `{ status: "run", mapId }`,
   `{ status: "pending" }`, or `{ status: "none" }`. This isolates the multi-key buffering logic — the part most worth
   unit-testing — from the DOM.
2. **`registerEmbedKeymapFallback()`** called from `onload`: mirrors the `task-status-cycler` precedent — capture-phase
   `keydown` on `window` + `document`, a `WeakSet` to dedupe, and `this.register(...)` teardown.
3. **`handleEmbedFallbackKeydown(event)`**: applies the guard (Behavior Spec 1–6), runs the matcher with the prefix
   buffer, and on a match `preventDefault`/`stopPropagation` + dispatches via a small map-id → action table.
4. **`resolveEmbedSourceLine(view, target)`** (Phase 2): walks from `target` to the embed block element and returns its
   0-based source line via `view.editor.cm.posAtDOM(...)`, or `null`.

## Testing & Verification

- **Unit (automated, plugin convention):** ad-hoc Node test with a mocked `obsidian` module exercising the pure matcher
  — single keys, both bracket sequences, prefix-then-mismatch, prefix-then-timeout, and the `<C-j>`/`<C-k>` chords. Run
  the plugin's existing helper-test command/pattern.
- **Manual (in-vault, required — DOM/focus can't be unit-tested under the mocked-module harness):** against a note with
  a **transcluded Bases table**, verify each map in three focus states:
  1. **Editor focused** (cursor in `.cm-editor`): all maps behave exactly as today (router stands down — no regression).
  2. **Embed selected, not editing** (table/cell focused, no input): `-`, `[[`, `]]`, `\|`, `<C-j>`, `<C-k>` fire; Phase
     2 `!`/`[<Space>`/`]<Space>` act on the embed's line.
  3. **Embed cell in edit mode** (input/contenteditable focused): the router stands down — typing `-`, `!`, `\|` inserts
     text normally and does **not** trigger Bob commands. Also sanity-check that focus in the file explorer / command
     palette / a sidebar never triggers a Bob command.

## Out of Scope / Risks

- Reading mode (no editor) is excluded.
- Vim count prefixes (e.g. `3<C-j>`) are excluded — the existing maps drop counts too.
- `posAtDOM` behavior over embed widgets is the main Phase 2 uncertainty; the stand-down-on-failure fallback bounds the
  blast radius (never mutates the wrong line). If it proves unreliable, Phase 2 can ship `!` only with explicit
  per-embed testing, or be deferred while Phase 1 (the core navigation win) still lands.
- Optional, not implemented here: binding `<C-j>`/`<C-k>` as native Obsidian hotkeys would additionally cover insert and
  reading modes; noted as a user-side enhancement.

## Deliverables

1. The router implementation in `bob-navigation-hotkeys/main.js` (Phase 1, then Phase 2) plus the exported pure matcher.
2. The unit test for the matcher.
3. No changes to `obsidian_vimrc.md` — the existing maps stay as the in-editor path; the router is purely additive for
   the out-of-editor case.
