---
create_time: 2026-06-19 22:58:15
status: done
prompt: sdd/prompts/202606/vim_surround_cs_keymap.md
---
# Plan: Add `cs{target}{replacement}` (change surround) to `bob-vim-surround`

## Goal

Add Tim Pope `vim-surround`-style **change surround** to the existing `bob-vim-surround` Obsidian plugin so that, in Vim
normal mode:

- `cs"'` on `"Hello world!"` → `'Hello world!'`
- `cs'(` on `'Hello'` → `( Hello )` (padding because `(` is the open form)
- `cs)]` on `(Hello)` → `[Hello]` (no padding because `)`/`]` are close forms)
- `cs([` on `( Hello )` → `[ Hello ]` (open-form target strips the old padding, open-form replacement re-adds it — net:
  no double spaces)

This is a **vault-side change** to `~/bob`, not a `bob-cli` (Rust) change. Same domain, constraints, and git discipline
as the prior plans `fix_vim_surround_ys_keymap.md` and `obsidian_vim_surround_keymaps_consolidated.md`.

## Context: what already exists

`~/bob/.obsidian/plugins/bob-vim-surround/main.js` already implements `ys{count}{motion}{char}` and ships a lot of
reusable machinery:

- An always-installed **capture-phase `keydown` listener** on `window`/`document`.
- Focused-Markdown-editor + **normal-mode Vim `cm` resolution** (`resolveEventNormalModeVimCm`, `getCurrentVimMode`).
- The **`y`→`s` interception** pattern: a bare `y` is recorded as a candidate and allowed to reach Vim; if the _next_
  key is `s`, the plugin cancels Vim's dangling yank-operator-pending with an injected `<Esc>` and takes over. Every
  other `y` command (`yy`, `yw`, `yiw`, `y$`, `"ayw`) is untouched.
- A **`SURROUND_PAIRS`** table keyed by delimiter char, each entry carrying `{ open, close, padded }` where open forms
  (`(` `[` `{` `<`) are `padded: true` and close forms (`)` `]` `}` `>`) and quotes are `padded: false`.
- Wrap/replace helpers: `operation` (single undo), `replaceRange`, `setCursor`, `consumeEvent`, and key-event guards
  (`getPlainLowercaseKeyFromEvent`, `getSurroundKeyFromEvent`, `isModifierOnlyKey`).

`cs` reuses all of the above. **Important dependency:** the `ys` fix is currently implemented but **uncommitted and not
yet verified in desktop Obsidian** (its acceptance is gated on a user-run dev-console spike). `cs` extends the _same_
shared interception code, so this work builds on the current working-tree state and must be verified in the same desktop
session as `ys`, without regressing it.

## Why `cs` is architecturally simpler than `ys` (and where it is harder)

`ys` needed Vim to parse an arbitrary motion (`2w`, `iw`, `i(`, `$`), so it had to inject a non-conflicting operator
(`<A-y>s`) and let Vim compute the span.

`cs` takes **exactly two literal characters** after the trigger — a _target_ delimiter and a _replacement_ delimiter —
and **no Vim motion at all**. So `cs` can be a **pure capture-phase state machine**: it needs Vim only to clear the
dangling change-operator-pending state via an injected `<Esc>` (the same `handleKey` call `ys` already relies on,
confirmed available). No new `defineOperator`, no motion injection, no dependency on the un-observed operator `ranges`
shape.

The one genuinely new, harder piece is a **surrounding-pair finder**: given a target delimiter and the cursor position,
locate the opening and closing delimiter that enclose the cursor. This is pure string logic over the buffer — which
means, unlike the `ys` key path, it is **fully unit-testable headlessly**. That property is deliberately central to this
plan (see Verification).

## Dispatch design (mirrors the proven `y`→`s` path)

`c` is Vim's change operator, so a plain `cs` mapping can never win against it (identical to why plain `ys` failed — `c`
dispatches immediately into operator-pending, and `s` is not a valid motion, so stock `cs` is a no-op). The fix is the
same interception tpope's design already assumes is free:

1. **Generalize the existing single-key candidate** from "yank candidate" to a small `{ cm, op }` trigger candidate
   where `op` is `'y'` or `'c'`. A bare `y` records `op:'y'`; a bare `c` records `op:'c'`. Both are _allowed to reach
   Vim_ (so `cc`, `cw`, `ciw`, `ci(`, `ca"`, `c$`, `"acw`, and all `y*` commands keep working untouched).
2. **On the next key:**
   - `op:'y'` + `s` → existing surround-add path (inject `<A-y>s`). Unchanged.
   - `op:'c'` + `s` → **change-surround path**: inject `<Esc>` to clear Vim's dangling change-pending, set
     `pendingChangeSurround = { cm, stage }`, and consume the `s` event. Because `cs` is an invalid no-op in stock Vim,
     we only ever repurpose the `c`+`s` combination.
   - anything else → clear the candidate, let Vim handle the key normally.
3. **Capture target then replacement via the existing capture-phase listener.** While `pendingChangeSurround` is set,
   the listener consumes the next two plain keys: first the **target** delimiter, then the **replacement** delimiter
   (exactly how `ys` already captures its trailing delimiter char).
   - Unknown target char, unknown replacement char, or `<Esc>` at either stage → cancel cleanly with **no edit** and
     stop consuming keys.

This keeps `cs` _less_ dependent on Vim internals than `ys`: the only Vim call is the already-proven `<Esc>` injection.

## Pair-finding semantics (the new core logic)

Resolve the target pair from the target char (reusing `SURROUND_PAIRS`), then find the enclosing pair around
`cm.getCursor()`:

- **Bracket pairs** (`()`, `[]`, `{}`, `<>`): balanced scan. Scan left from the cursor counting depth (a close
  increments depth, an open at depth 0 is the match) to find the opening delimiter; scan right for the matching close.
  May span lines, with a bounded iteration cap to avoid pathological cost. If the cursor sits on a delimiter, treat that
  delimiter as the corresponding end.
- **Quote delimiters** (`"`, `'`, `` ` ``): quotes do not nest, so scan the **current line** only — nearest target char
  at/left of the cursor is the open, nearest target char to the right is the close.
- **Not found** (no enclosing pair) → no-op, exit pending state, do not swallow later keys.

### Padding / whitespace rule (tpope-aligned, explicit for v1)

Padding is symmetric between target and replacement:

- **Replacement** padding follows `SURROUND_PAIRS[replacement].padded` — open forms add one inner space each side, close
  forms and quotes add none.
- **Target** removal strips one inner whitespace char adjacent to each delimiter **iff the target was given in its open
  form** (`(` `[` `{` `<`); close forms and quotes strip nothing. This prevents double-spacing when converting a padded
  pair to another padded pair and matches tpope (`cs([` on `( Hello )` → `[ Hello ]`).

Inner content is otherwise preserved verbatim. The replacement is applied in a single `cm.operation` (one undo): replace
the closing delimiter span first (later position), then the opening delimiter span, then place the cursor on/after the
new opening delimiter — consistent with the existing `applySurround`.

## Scope

**In scope (v1):** `cs{target}{replacement}` for the existing v1 delimiter set (`" ' \` ( ) [ ] { }
< >`) in normal mode, with the padding rule above, balanced bracket finding (incl. nested/multi-line) and single-line quote finding, single undo, `<Esc>`/unknown-key
cancel, and headless unit tests of the finder + apply.

**Out of scope (unchanged from prior plans):** `ds` (delete surround), visual-mode `S`, HTML/tag targets and
replacements (`cst`, `<q>`), wiki-link / markdown-emphasis wrappers and aliases (`b`/`B`/`r`), counts, and dot-repeat.
These can follow once `cs` is verified.

## Alternative considered (documented fallback)

Instead of a custom finder, inject `vi{target}` to make Vim select the inner text object, read `cm.listSelections()`,
and expand by one delimiter each side. This reuses Vim's battle-tested matcher (and CodeMirror Vim does provide
`i(`/`i"`/etc.), and `obsidian-vimrc-support` has precedent for reading selections. **Rejected as primary** because it
cannot be unit-tested headlessly and adds dependency on the same un-observed selection shape the `ys` plan flagged. Kept
as the fallback if the custom finder proves unreliable on real Markdown (e.g. quotes interacting with formatting); the
desktop spike can capture the selection shape if we switch.

## Verification (carrying the `ys` process lesson)

The `ys` task shipped a broken key path because the only test exercised the wrap helper, never the dispatch. This plan
splits verification accordingly:

1. **Headless unit tests (must pass, gate the logic):** a Node test with mocked `cm` exercising the pair-finder + apply
   across: `cs"'`, `cs'(` (pad), `cs)]` (no pad), `cs([` on `( Hello )` (strip+pad, no double space), nested brackets
   `cs)]` on `([Hello])`, multi-line bracket pair, cursor-on-delimiter, and not-found (no edit). Also a mocked-dispatch
   test that `c`→`s` enters change-pending and that `cc`/`cw`/lone-`c`-then-Esc/lone-`s` still fall through.
2. **Desktop Obsidian acceptance (the real key path — user-run gate):** because the agent cannot drive desktop Obsidian,
   hand the user a copy-paste dev-console spike + manual checklist and treat their confirmation as the acceptance gate.
   Do **not** declare success from the Node simulation alone.

Manual acceptance checklist (desktop, Vim mode, Markdown editor):

- `cs"'`, `cs'"`, `cs"\`` between the three quotes.
- `cs"(` → `( … )` vs `cs")` → `(…)`; `cs([` on `( Hello )` → `[ Hello ]`.
- `cs)]`, `cs}]`, `cs>)` across bracket types; nested `([Hello])` with `cs)]`.
- `<Esc>` after `cs` (awaiting target or replacement) cancels with no edit.
- Cursor not inside any target pair → no-op, no stray edit.
- **Regressions:** `cc`, `cw`, `ciw`, `ci(`, `ca"`, `c$`, `"acw`, lone `s` (substitute), and the full `ys` set (`ys3w"`,
  `ysiw"`, `yy`, `yw`, `y$`) still work; lone `c` then `<Esc>` is a clean cancel. Existing vimrc maps (`[[`, `]]`, `-`,
  `!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, `o`/`O`) unaffected.

## Files touched

- `~/bob/.obsidian/plugins/bob-vim-surround/main.js` — add the `c`-candidate branch, `pendingChangeSurround` state
  machine, pair-finder, and change-apply.
- `~/bob/.obsidian/plugins/bob-vim-surround/manifest.json` — update the description (currently "ys motions") to include
  `cs`; bump version to `1.1.0`.

No `obsidian_vimrc.md`, no `supportJsCommands`, no `bob-cli` Rust changes.

## Implementation steps

1. **Generalize the trigger candidate** to `{ cm, op }` and add the `c` branch alongside `y`, preserving the existing
   `ys` dispatch exactly.
2. **Add the `pendingChangeSurround` state machine** (await-target → await-replacement → apply / cancel) wired into the
   existing capture-phase `handleSurroundKeydown` dispatch, with `<Esc>`/unknown-key cancel.
3. **Implement the pair-finder** (balanced bracket scan + single-line quote scan, bounded, cursor-on-delimiter handling)
   and the **change-apply** (target-strip + replacement-pad rule, single `cm.operation`, cursor placement).
4. **Update `manifest.json`** description + version.
5. **Write headless Node unit tests** for the finder + apply + mocked dispatch (step 1 of Verification). Run
   `node --check` and `git -C ~/bob diff --check` on the touched file.
6. **Verify the real key path** via the desktop dev-console spike + manual checklist (step 2), verifying `cs` and
   re-verifying `ys` in the same session. Do not accept the Node simulation as proof of the key path.
7. **Commit** only the touched vault file(s) via the `sase_git_commit` workflow, leaving unrelated vault dirt untouched
   (never `git add -A` in `~/bob`).
