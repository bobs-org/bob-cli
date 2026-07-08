---
create_time: 2026-07-07 20:33:41
status: wip
prompt: sdd/prompts/202607/fix_counted_transclusion_keymaps.md
---
# Plan: Fix Non-Working Counted `!` / `@` Transclusion Keymaps

## Goal

The previously-shipped "counted transclusion keymaps" feature does not work: `2!` and `2@` behave exactly like bare `!`
/ `@` (single line only). Diagnose and fix the root cause so that an explicit Vim count (`N!` / `N@`) once again means
"current line plus `N` following source lines", per the original approved feature plan.

## Root Cause (Confirmed)

Both counted paths are gated on a single helper, `getPendingVimRepeat(cm)`, which is duplicated in each plugin:

- `plugins/bob-navigation-hotkeys/main.js` (counted `!` capture listener →
  `handleCountedTransclusionTogglePhysicalKeydown`)
- `plugins/task-status-cycler/main.js` (counted `@` capture listener → `dispatchTranscludedTaskStartEvent`)

That helper reads the pending count via `cm.state.vim.inputState.getRepeat()`. **This is the wrong field at the moment
it is read**, which makes the helper always report `{ repeat: 1, explicit: false }`, so both handlers silently fall back
to their single-line behavior. That is exactly the "didn't work at all" symptom.

Why `getRepeat()` is always 0 here (verified against the authoritative codemirror-vim source that Obsidian's Vim mode
derives from):

1. The counted `!` / `@` handlers are **capture-phase** `keydown` listeners on `window`/`document`. Capture phase runs
   **before** CodeMirror/Vim's own key handling on the editor content DOM.
2. In CodeMirror-Vim, when the user types a count digit like `2`, it is pushed as an unparsed string into
   `inputState.keyBuffer` (an array, e.g. `["2"]`) and Vim returns early, waiting for more keys. The digit is **not**
   yet in `prefixRepeat`.
3. `InputState.getRepeat()` reads **only** `prefixRepeat` / `motionRepeat`. Those are populated by `pushRepeatDigit()`,
   which Vim calls **only after a full command match completes** — i.e. only once Vim itself processes the following `!`
   / `@` key.
4. Our capture listener reads `getRepeat()` for the `!` / `@` keydown _before_ Vim ever processes it. At that instant
   `prefixRepeat` is still empty and the count is still sitting in `keyBuffer`. So `getRepeat()` returns `0` →
   `explicit: false` → single-line fallback, every time.

This also explains why the earlier automated "helper checks" passed while the feature was broken in practice: those
stubs mocked `inputState.getRepeat()` returning the count, which does not reflect real capture-phase timing. The stubs
validated an incorrect model of the runtime.

Corroborating evidence that the Vim engine itself does track counts correctly (so this is purely a "read the wrong place
at the wrong time" bug, not a missing-count bug): `task-status-cycler` already registers real Vim actions via
`window.CodeMirrorAdapter.Vim.defineAction(...)` + `Vim.mapCommand("<CR>", "action", ...)`, and those receive an
accurate `actionArgs.repeat` / `actionArgs.repeatIsExplicit` — because they run _inside_ Vim's command processing, after
`pushRepeatDigit()`.

## Context Reviewed

- Re-read Obsidian long-term memory via the memory-read skill.
- Opened the linked `bob-plugins` source repo (edits belong there, then deployed with `bob plugins sync`).
- Read the shipped feature commit and both plugin `main.js` files:
  - `getPendingVimRepeat` / `resetPendingVimInputState` (duplicated in both plugins).
  - Counted `!` listener registration and handler in `bob-navigation-hotkeys` (confirmed registered in `onload`, so the
    listener is live — the only failure point is the count read).
  - Counted `@` dispatch and `toggleTranscludedTaskStartStateRange` in `task-status-cycler`.
  - The proven `defineAction` / `mapCommand` count path already used for `<CR>` navigation.
  - `resolveVimCodeMirror` / `resolveNormalModeVimCm` — both return the CodeMirror-Vim CM5 adapter (`editor.cm.cm`),
    which does expose the live `cm.state.vim` (mode detection already relies on it), so `inputState` is reachable.
- Confirmed the live vimrc mapping `nmap ! :bob_toggle_transclusions<CR>` (bare `!`, count-less via `obcommand`).
- Verified against the upstream codemirror-vim `InputState` source: `keyBuffer` (array) holds pending count digits;
  `getRepeat()` reads only `prefixRepeat`/`motionRepeat`; `pushRepeatDigit()` runs only after a full command match.
- No `bob-cli` subcommands/options change, so `cli_rules.md` is not required.

## Fix Approach

Keep the existing architecture (capture-phase physical listeners for both `!` and `@` — required for `@` so ordinary Vim
`@<reg>` macro playback still falls through). The change is surgical: **read the pending count from where it actually
lives at capture time.**

### 1. Rewrite `getPendingVimRepeat(cm)` in both plugins

Derive the pending count from `inputState.keyBuffer` (the unparsed leading digits) instead of `getRepeat()`:

- Join `keyBuffer` to a string, defensively supporting both array and string representations (Obsidian forks have
  historically used either).
- Parse a leading Vim count with `/^([1-9]\d*)/` (a leading `0` is the start-of-line motion, not a count — must not be
  treated as one).
- Keep `getRepeat()` only as a secondary fallback for any path where a count already reached `prefixRepeat`.
- Return `{ repeat, explicit: true }` when a positive count is found, else `{ repeat: 1, explicit: false }`.

This one helper fix restores both counted `!` and counted `@`, since both consume the same helper.

### 2. Ensure the pending count is cleared after consuming a counted key

Because the counted handlers `preventDefault()` + `stopImmediatePropagation()` the `!` / `@` keydown at capture, Vim
never processes that key and therefore never clears its own `keyBuffer` (which still holds the count digits). The
existing `resetPendingVimInputState(cm)` must reliably clear `keyBuffer` (and `prefixRepeat`/`motionRepeat`) so the
count does not leak into the next Vim command (e.g. a following `j` moving twice).

- Confirm/tighten `resetPendingVimInputState` so it definitively empties `keyBuffer` on the live `inputState`.
- Prefer clearing fields on the existing `inputState` object over swapping in a brand-new instance, to avoid disturbing
  any per-editor state Vim may hold; keep the swap only as a fallback.

### 3. No change to bare behavior

- Bare `!` (no digits): `keyBuffer` is empty → `explicit: false` → handler returns without `preventDefault`, so `!`
  propagates to the existing `nmap ! :bob_toggle_transclusions<CR>` (single line) — unchanged.
- Bare `@` (no digits): `explicit: false` → existing single-line active-line path — unchanged.
- Non-eligible active line for `@`: still returns before `preventDefault`, so `@<reg>` macro playback still works.

### Alternative considered (not chosen)

Re-implement counted `!` as a real `Vim.defineAction` + `Vim.mapCommand("!", ...)` (the proven `<CR>` path, which gets
`actionArgs.repeat` reliably). Rejected as the primary fix because:

- `@` cannot use a plain normal-mode action mapping without breaking Vim macro playback (`@` is the macro key), so it
  must keep the capture-phase approach regardless — unifying both on the `keyBuffer` read is simpler and consistent.
- It would require editing `obsidian_vimrc.md` (removing the bare `nmap !`) and folding the bare-toggle command logic
  into the action, a larger change than the confirmed root cause requires.

This alternative can be revisited later as a `!`-only robustness improvement if desired.

## Deployment

Edit only the linked `bob-plugins` source files:

- `plugins/bob-navigation-hotkeys/main.js`
- `plugins/task-status-cycler/main.js`

Then deploy per plugin (this CLI takes one `--plugin` per invocation):

```bash
bob plugins sync --plugin bob-navigation-hotkeys --repo "<linked bob-plugins source path>"
bob plugins sync --plugin task-status-cycler --repo "<linked bob-plugins source path>"
```

Do not edit the deployed copies under the vault directly.

## Acceptance Criteria

- With the cursor on a block-link line followed by two block-link lines, `2!` toggles transclusion syntax on all three
  lines (one bulk mode across the range), and `2@` toggles the source-task start state for all three eligible targets.
- Bare `!` and bare `@` still operate on only the current line.
- `@<register>` macro playback still works on ordinary (non-eligible) lines, with and without a count.
- After a handled `2!` / `2@`, the next Vim command (e.g. `j`) is not double-counted — no residual count leaks.
- `<Ctrl+Enter>`, `<Enter>`, Backspace, open-task jumps, and existing vimrc mappings are unchanged.

## Verification Plan

Static checks from the `bob-plugins` source repo:

```bash
npm run validate
node --check plugins/bob-navigation-hotkeys/main.js
node --check plugins/task-status-cycler/main.js
git diff --check
```

Focused helper checks — **modeling the real runtime, not the previous incorrect stub**:

- `getPendingVimRepeat`:
  - `inputState.keyBuffer === ["2"]` with `getRepeat() === 0` → `{ repeat: 2, explicit: true }` (this is the exact case
    the old code got wrong).
  - `keyBuffer === []` with `getRepeat() === 0` → not explicit.
  - `keyBuffer === ["0"]` → not explicit (leading `0` is a motion, not a count).
  - Multi-digit `keyBuffer === ["1","2"]` → `repeat: 12`, explicit.
  - String-form `keyBuffer === "2"` (fork-compat) → `repeat: 2`, explicit.
  - `prefixRepeat`-only fallback (`getRepeat() === 3`, empty `keyBuffer`) → `repeat: 3`, explicit.
- `resetPendingVimInputState`: empties `keyBuffer`/`prefixRepeat` without throwing.

Manual smoke in Obsidian after `bob plugins sync` + reload (this is the authoritative check — the failure is a runtime
event-timing issue that unit stubs alone cannot catch):

1. Three adjacent bare task block-link lines, press `2!` → all three become embedded; `2!` again → all three bare.
2. Three adjacent embedded task block-link lines on open source tasks, press `2@` → all three source tasks go
   in-progress; `2@` again → back to open.
3. On an ordinary line, run `@<register>` and `2@<register>` → macro playback still works.
4. After a handled `2!` / `2@`, press `j` → cursor moves exactly once.
5. Bare `!` / bare `@` on a single line → single-line behavior unchanged.

## Risks and Mitigations

- Risk: reading `inputState.keyBuffer` depends on Vim internals.
  - Mitigation: it is the same `cm.state.vim.inputState` object the plugins already depend on for mode detection and
    counted actions; handle array/string forms defensively and keep `getRepeat()` as a fallback.
- Risk: residual count leaks into the next command after consuming a counted key.
  - Mitigation: reliably clear `keyBuffer` in `resetPendingVimInputState`; cover with manual step 4.
- Risk: over-trusting automated stubs again.
  - Mitigation: rewrite the focused checks to model real capture-phase state (count in `keyBuffer`,
    `getRepeat() === 0`), and require the in-Obsidian manual smoke as the final gate.
- Risk: source/deployed drift.
  - Mitigation: edit linked source only, deploy with `bob plugins sync`, verify targeted diffs after deployment.
