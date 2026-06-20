---
create_time: 2026-06-19 23:11:32
status: wip
prompt: sdd/prompts/202606/vim_surround_ds_keymap.md
---
# Plan: Add `ds{target}` (delete surround) to `bob-vim-surround`

## Goal

Add Tim Pope `vim-surround`-style **delete surround** to the existing `bob-vim-surround` Obsidian plugin so that, in Vim
normal mode, the enclosing delimiter pair around the cursor is removed:

- `ds"` on `"Hello world!"` → `Hello world!`
- `ds)` on `(Hello)` → `Hello`
- `ds(` on `( Hello )` → `Hello` (open-form target strips the inner padding)
- `ds)` on `( Hello )` → `Hello` (close-form target preserves the inner spaces)
- `ds)` on `([Hello])` → `[Hello]` (removes the **nearest enclosing** `()` pair)

This is a **vault-side change** to `~/bob`, not a `bob-cli` (Rust) change. Same domain, constraints, and git discipline
as the prior surround plans (`fix_vim_surround_ys_keymap.md` and `vim_surround_cs_keymap.md`).

## Context: what already exists

The `cs` work just landed (commit `3bafa34 feat(obsidian): add vim-surround cs keymap`), so
`~/bob/.obsidian/plugins/bob-vim-surround/main.js` already ships **almost everything `ds` needs**:

- The always-installed **capture-phase `keydown` listener**, focused-Markdown-editor + normal-mode Vim `cm` resolution
  (`resolveEventNormalModeVimCm`, `getCurrentVimMode`), and the **`<Esc>` injection** primitive (`injectVimKey`).
- The generalized **`{ cm, op }` trigger candidate** already handling `op: 'y'` (surround-add) and `op: 'c'`
  (change-surround), recorded on a bare `y`/`c` and allowed to reach Vim so every other `y*`/`c*` command is untouched.
- The **`SURROUND_PAIRS`** table (`{ open, close, padded }`, open forms padded) and **`SURROUND_PAIRS`-driven
  pair-finder** `findEnclosingSurroundPair(cm, targetKey)` — balanced bracket scan (nested + multi-line, bounded by
  `MAX_SURROUND_SCAN_CHARS`, cursor-on-delimiter aware) plus single-line quote scan. **This is exactly the finder `ds`
  needs, already unit-tested for `cs`.**
- `buildChangeSurroundEdit(match, replacementPair)` — which already contains the **target-padding-strip rule**
  (`if (match.targetPair.padded)` strips one horizontal-whitespace char inside each delimiter). `ds` needs precisely
  this strip, with empty replacement text.
- The single-undo apply scaffolding (`operation`, `replaceRange`, `setCursor`), key-event guards
  (`getSurroundKeyFromEvent`, `getPlainLowercaseKeyFromEvent`, `isModifierOnlyKey`), and `consumeEvent`.

**Dependency note:** `ys` and `cs` are committed but their **desktop-Obsidian acceptance is still a pending user gate**
(the agent cannot drive the GUI; this machine runs `obsidian-headless` via `ob`). `ds` extends the _same_ shared
interception + finder code, so this work builds on the current committed tree and should be **verified in the same
desktop session as `cs`/`ys`, without regressing them.**

## Why `ds` is the simplest of the three operators

`ys` had to inject a non-conflicting operator (`<A-y>s`) and let Vim parse an arbitrary motion. `cs` added a
two-character target→replacement state machine plus the pair-finder. `ds` is **`cs` minus the replacement**:

- It takes **exactly one literal character** after the trigger — a _target_ delimiter — and **no Vim motion**.
- It reuses the **already-built, already-tested** pair-finder and target-padding-strip verbatim.
- Its only Vim dependency is the proven `<Esc>` injection (to clear Vim's dangling delete-operator-pending), identical
  to `cs`.

There is **no new core algorithm** — only a one-stage capture, an empty-delimiter apply, and one more `op` value in the
existing trigger candidate.

## Dispatch design (mirrors the proven `c`→`s` path)

`d` is Vim's delete operator, so a plain `ds` mapping can never win against it — and `s` is not a valid motion, so stock
`ds` is a **no-op** in Vim (exactly the property that made repurposing `c`+`s` and `y`+`s` safe). The interception:

1. **Add `'d'` to the trigger candidate.** A bare `d` (in normal mode, focused Markdown editor) records `op: 'd'` and is
   **allowed to reach Vim** — so `dd`, `dw`, `diw`, `di(`, `da"`, `d$`, `dt.`, `D`, `dj`, `"add`, etc. keep working
   untouched (an existing candidate is cleared the moment the next key is not `s`, completing repeated/normal operators
   like `dd`/`dw` exactly as `cc`/`cw` already do today).
2. **On the next key, `op: 'd'` + `s`** → **delete-surround path**: inject `<Esc>` to clear the dangling
   delete-operator-pending, set a **one-stage** `pendingDeleteSurround = { cm }`, and consume the `s` event. (Anything
   other than `s` → clear the candidate, let Vim handle it normally.)
3. **Capture the single target delimiter** via the existing capture-phase listener (the same way `cs` captures its first
   delimiter). Unknown target char or `<Esc>` → cancel cleanly with **no edit** and stop consuming keys.

This keeps `ds` no more Vim-coupled than `cs`: the only Vim call is the already-proven `<Esc>` injection.

## Delete semantics (reusing the `cs` finder + strip rule)

Resolve the target pair from `SURROUND_PAIRS`, find the enclosing pair around `cm.getCursor()` with the **existing**
`findEnclosingSurroundPair`, then delete:

- **Brackets** (`()` `[]` `{}` `<>`): balanced scan finds the nearest enclosing pair of that type (so nested `ds)` on
  `([Hello])` removes the outer parens → `[Hello]`); may span lines; cursor-on-delimiter handled.
- **Quotes** (`"` `'` `` ` ``): single-line nearest-pair scan (quotes don't nest).
- **Not found** → no-op, exit pending state, do not swallow later keys.

**Padding rule (consistent with `cs`, the key design choice):** the target delimiter's `padded` flag drives inner-space
removal — **open forms** (`(` `[` `{` `<`) strip one adjacent inner whitespace char on each side; **close forms and
quotes** strip nothing. This is the _same_ `targetPair.padded` strip already implemented and tested in
`buildChangeSurroundEdit`, so `ds`'s whitespace behavior is predictable and matches `cs`:

- `ds(` on `( Hello )` → `Hello`
- `ds)` on `( Hello )` → `Hello`

Inner content is otherwise preserved verbatim. The edit runs in a **single `cm.operation`** (one undo): delete the
closing-delimiter span first (later position), then the opening-delimiter span, then place the cursor at the start of
the now-unwrapped content (where the opening delimiter was) — consistent with the existing `applySurround` /
`applyChangeSurround` ordering.

### Reuse / simplification

Rather than copy the strip logic, **factor the shared target-span computation (open/close padding-strip) out of
`buildChangeSurroundEdit`** into a small helper, and add a `buildDeleteSurroundEdit(match)` that reuses it and emits
**empty** opening/closing text with the cursor at the opening span's start. `cs` keeps adding replacement delimiters;
`ds` adds none. This avoids divergence between the two whitespace rules.

## Scope

**In scope (v1):** `ds{target}` for the existing v1 delimiter set (`" ' \` ( ) [ ] { }
< >`) in normal mode, the padding-strip rule above, balanced bracket finding (incl. nested / multi-line) and single-line quote finding, single undo, `<Esc>`
/ unknown-key cancel, and headless unit tests of the delete-apply + dispatch.

**Out of scope (unchanged from prior plans):** visual-mode mappings, HTML/tag targets (`dst`, `<q>`), wiki-link /
markdown-emphasis wrappers and aliases (`b`/`B`/`r`), counts, and dot-repeat. These can follow once `ds` is verified.

## Alternative considered (documented fallback)

Inject `di{target}` to make Vim select the inner text object, read `cm.listSelections()`, and expand by one delimiter
each side to delete. Reuses Vim's battle-tested matcher. **Rejected as primary** for the same reasons as `cs`: it cannot
be unit-tested headlessly and depends on the un-observed selection shape the `ys` plan flagged. Kept as the fallback if
the custom finder proves unreliable on real Markdown; the shared desktop spike can capture the selection shape if we
switch. (Since `ds` reuses the already-verified-for-`cs` finder, the fallback is unlikely to be needed.)

## Verification (carrying the `ys` process lesson)

The `ys` task shipped a broken key path because its only test exercised the wrap helper, never the dispatch. Split
verification accordingly:

1. **Headless unit tests (must pass, gate the logic):** a Node test with mocked `cm` exercising the (reused) finder +
   new delete-apply across: `ds"`, `ds'`, `ds)` on `(Hello)`, `ds(` on `( Hello )` (strip), `ds)` on `( Hello )` (no
   strip), nested `ds)` on `([Hello])` → `[Hello]`, multi-line bracket pair, cursor-on-delimiter, and not-found (no
   edit). Plus a mocked-dispatch test that `d`→`s` enters delete-pending and that `dd` / `dw` / lone-`d`-then-`<Esc>` /
   lone-`s` still fall through — and that the existing `cs` / `ys` dispatch is unaffected.
2. **Desktop Obsidian acceptance (the real key path — user-run gate):** the agent cannot drive desktop Obsidian, so hand
   the user a copy-paste dev-console spike + manual checklist and treat their confirmation as the acceptance gate. Do
   **not** declare success from the Node simulation alone.

Manual acceptance checklist (desktop, Vim mode, Markdown editor):

- `ds"`, `ds'`, ``ds` `` between the three quote types.
- `ds)` / `ds(` on `(Hello)` and on `( Hello )` (confirm the open-form-strips-padding behavior matches your tpope
  expectation); `ds]`, `ds}`, `ds>` across bracket types; nested `([Hello])` with `ds)` → `[Hello]`.
- `<Esc>` after `ds` (awaiting target) cancels with no edit; cursor not inside any target pair → no-op, no stray edit.
- **Regressions:** `dd`, `dw`, `diw`, `di(`, `da"`, `d$`, `dt.`, `D`, `"add`, lone `s` (substitute), and the full `cs`
  set (`cs"'`, `cs([`, `cs)]`) and `ys` set (`ys3w"`, `ysiw"`, `yy`, `yw`, `y$`) still work; lone `d` then `<Esc>` is a
  clean cancel. Existing vimrc maps (`[[`, `]]`, `-`, `!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, `o`/`O`)
  unaffected.

## Files touched

- `~/bob/.obsidian/plugins/bob-vim-surround/main.js` — add the `'d'` candidate branch, the one-stage
  `pendingDeleteSurround` state machine, the factored target-span helper + `buildDeleteSurroundEdit`, and the
  delete-apply; expose `buildDeleteSurroundEdit` on the `__test` hook.
- `~/bob/.obsidian/plugins/bob-vim-surround/manifest.json` — update the description (currently "ys motions and cs
  changes") to include `ds`; bump version to `1.2.0`.

No `obsidian_vimrc.md`, no `supportJsCommands`, no `bob-cli` Rust changes.

## Implementation steps

1. **Add the `'d'` trigger branch** alongside `y`/`c` in the trigger-candidate recorder, preserving the existing `ys`
   and `cs` dispatch exactly.
2. **Add the one-stage `pendingDeleteSurround` state machine** (await-target → apply / cancel) wired into the existing
   capture-phase dispatch, reusing the same `<Esc>` / unknown-key cancel handling as the change path.
3. **Factor the target-span / padding-strip out of `buildChangeSurroundEdit`** and add `buildDeleteSurroundEdit(match)`
   (empty delimiters, cursor at content start) + the delete-apply (single `cm.operation`, reusing
   `findEnclosingSurroundPair`).
4. **Update `manifest.json`** description + version (`1.2.0`).
5. **Write headless Node unit tests** for the delete-apply + mocked dispatch (Verification step 1). Run `node --check`
   and `git -C ~/bob diff --check` on the touched file.
6. **Verify the real key path** via the desktop dev-console spike + manual checklist (Verification step 2), verifying
   `ds` and re-verifying `cs`/`ys` in the same session. Do not accept the Node simulation as proof of the key path.
7. **Commit** only the touched vault file(s) via the `sase_git_commit` workflow, leaving unrelated vault dirt untouched
   (never `git add -A` in `~/bob`).
