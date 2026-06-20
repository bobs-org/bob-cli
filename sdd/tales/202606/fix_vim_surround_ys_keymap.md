---
create_time: 2026-06-19 22:37:43
status: wip
prompt: sdd/prompts/202606/fix_vim_surround_ys_keymap.md
---
# Plan: Fix `bob-vim-surround` so `ys{count}{motion}{char}` actually fires

## Problem statement

The `bob-vim-surround` plugin (added in the prior task, vault path `~/bob/.obsidian/plugins/bob-vim-surround/`) does not
work. Typing `ys2w` in normal mode just moves the cursor two words forward instead of entering a surround-pending state.
The canonical acceptance case `ys3w"` → `Some "piece of text" goes here.` does not happen at all.

This plan diagnoses the root cause and fixes it. It is a **vault-side change** (`~/bob`), not a `bob-cli` (Rust) change
— same domain and constraints as the original plan `obsidian_vim_surround_ys_1.md`.

## Root cause (diagnosed, with evidence)

The plugin registers `ys` as a Vim operator:

```js
// ~/bob/.obsidian/plugins/bob-vim-surround/main.js:305
vim.mapCommand("ys", "operator", SURROUND_OPERATOR_NAME, {}, { context: "normal" });
```

**This can never fire, because `y` is already a complete normal-mode operator in CodeMirror's Vim.**

CodeMirror's Vim key dispatcher matches the typed key buffer against its keymap and classifies each command as a _full_
or _partial_ match. When the user presses `y`:

- `y` (built-in yank operator) is a **full** match → it is dispatched **immediately**, putting Vim into operator-pending
  state.
- `ys` (our mapping) is only a **partial** match at that instant.

The dispatcher prefers the existing full match and never waits to see whether a longer mapping (`ys`) would complete. So
our operator is unreachable. What the user actually triggers with `ys2w` is:

1. `y` → yank operator pending.
2. `s` → not a valid motion in operator-pending state → the pending yank is discarded.
3. `2w` → runs as an ordinary normal-mode motion → **cursor jumps two words ahead.** (Exactly the reported symptom.)

### Evidence gathered

- **The trivial causes are ruled out.** The plugin is enabled in `~/bob/.obsidian/community-plugins.json`, its `main.js`
  passes `node --check`, and `~/bob/obsidian_vimrc.md` contains no `y`/`s`/`ys` mapping that could collide. The plugin
  loads and registers; the _mapping strategy_ is what fails.
- **A working reference confirms the constraint.** `obsidian-vimrc-support` ships its own surround operator and maps it
  to `<A-y>s` (Alt-y, then s) — **never plain `ys`** (`main.js:1003`). That author deliberately used an Alt-prefixed key
  specifically because a plain `y`-prefixed two-key operator cannot win against the built-in `y` operator. Its operator
  callback also reads a _visual selection_ rather than trusting the operator `ranges` argument.
- **The original plan anticipated this risk but it was never validated.** The plan called for a runtime spike to confirm
  `defineOperator` + operator `ranges`; the implementer could not drive desktop Obsidian and shipped without running it.
  The "verification" that passed was a Node simulation of the _wrap helper_ (`applySurround` fed hand-built ranges) —
  which never exercised the `ys` key-matching path at all. That is the secondary, process root cause: the broken path
  was the one piece that was never actually tested.

## Secondary unknown to resolve during the fix

`obsidian-vimrc-support` does **not** rely on the operator `ranges` argument (it reads a visual selection instead), so
this Obsidian build has _no proven example_ of a custom `defineOperator` callback receiving a correct motion range. Our
current `main.js` assumes it does, with a large, speculative `collectRawRanges`/`normalizeRange` block that "guesses" at
several possible shapes — a strong smell that the range shape was never observed. The fix must **empirically establish
the real range shape** (or avoid depending on it), not keep guessing.

## Fix strategy

Two architectures are viable. The motion still must be parsed by Vim (re-implementing count + text-object parsing in a
DOM handler is exactly why the off-the-shelf "More Vim" option is limited). The decision is _how_ to get Vim to parse
the motion now that a plain `ys` operator is impossible.

### Primary approach — intercept physical `ys`, re-dispatch to a non-conflicting Vim operator

Keep the operator-based design (best motion fidelity, including text objects like `ysi(`), but stop relying on Vim to
match `ys`:

1. **Map the operator under a non-conflicting trigger** that Vim _can_ match as a multi-key command — e.g. `<A-y>s`
   (proven mappable by `obsidian-vimrc-support`) or another sequence whose prefix is not itself a standing command.
2. **Translate physical `ys` → the trigger at the capture phase.** Our always-installed capture-phase `keydown` listener
   watches for the `y`→`s` transition in normal mode: when a bare `y` (no Ctrl/Alt/Meta) is immediately followed by `s`,
   swallow the `s`, clear Vim's now-dangling yank-operator-pending state (inject `<Esc>` via
   `window.CodeMirrorAdapter.Vim.handleKey(cm, "<Esc>")` — `handleKey` is confirmed available; `obsidian-vimrc-support`
   uses it at `main.js:612`), then inject the trigger keys so Vim enters operator-pending for **our** operator.
   - Letting the bare `y` reach Vim normally and only hijacking the following `s` means every other `y` command (`yy`,
     `yw`, `yiw`, `y$`, `"ayw`, counts) keeps working untouched; we only repurpose the `y`+`s` combination, which is an
     invalid no-op in stock Vim anyway (this is precisely tpope's `ys` design).
3. **Let the real motion keys flow to Vim**, which parses `2w` / `3w` / `iw` / `i(` / `$` / `e` etc. and invokes our
   operator with the computed span.
4. **Capture the trailing delimiter char** with the existing capture-phase mechanism and perform the wrap (the wrap
   semantics already implemented — trailing-whitespace trim, open/close padding, single-undo, cursor — are fine and can
   be largely retained).

**Mandatory blocking spike (resolves the secondary unknown).** Before building this out, run a dev-console spike in
desktop Obsidian to confirm, for the chosen trigger: (a) injecting the trigger via `handleKey` then a real motion fires
our operator, and (b) what the operator callback actually receives as the motion span. If the `ranges` argument is
reliable, keep using it and **delete the speculative `collectRawRanges` guessing** in favor of the real shape. If it is
_not_ reliable, switch the operator to make Vim select the motion (inject `v{motion}` and read `cm.listSelections()` /
the Obsidian `editor` selection) — the proven `obsidian-vimrc-support` pattern.

### Fallback approach — self-parsed motion grammar (DOM state machine)

If the spike shows the operator/injection path is unworkable in this build, fall back to a fully self-contained DOM
state machine (the "More Vim"-style approach the original plan named as plan B): after intercepting `ys`, parse a small
explicit grammar ourselves — optional count + one of `w`/`e`/`b`/`W`/`E`/`B`/`iw`/`aw`/`$`/`0`/`^` (and, if feasible,
`i{pair}`/`a{pair}`) — then the delimiter char. This depends on the fewest Obsidian internals and is fully unit-testable
without a live Obsidian, at the cost of a bounded motion set. It covers the user's actual reported need (`ys2w`,
`ys3w"`) directly.

**Recommendation:** primary approach, gated on the spike, with the fallback ready if the spike fails. Either way the
wrap semantics and v1 delimiter set are identical to the original plan.

## Closing the verification gap (most important process change)

The original implementation shipped because the only test exercised the wrap helper, not the key path. This fix is **not
done until the actual `ys` key path is verified in running desktop Obsidian**, not just in a Node simulation. Because
the agent environment cannot drive desktop Obsidian, the plan must treat verification as a first-class, explicit step:

- Run the dev-console spike (above) and record what it actually returns.
- Perform the manual acceptance checks below in desktop Obsidian.
- If the agent cannot run desktop Obsidian, **hand the user a short, copy-pasteable dev-console spike snippet and the
  manual checklist, and treat their confirmation as the acceptance gate** — do not declare success from a Node
  simulation again.

## Acceptance / verification

Primary acceptance (must pass in desktop Obsidian):

- `Some piece of text goes here.`, cursor on `p` of `piece`, `ys3w"` → `Some "piece of text" goes here.`
- `ys2w` enters surround-pending (does **not** move the cursor two words); typing the delimiter completes the wrap;
  `<Esc>` while pending cancels with no edit.

Motion + delimiter coverage: `ysiw"`, `ysaw"`, `ys2w(` (inner padding) vs `ys2w)` (no padding), `yse"`, `ys$"`, and each
of `" ' ` ( ) [ ] { } < >` with correct spacing.

Regression (must still work): `yy`, `yw`, `yiw`, `y$`, `"ayw`, counts, and the existing vimrc maps (`-`, `[[`, `]]`,
`!`, `[<Space>`, `]<Space>`, `<C-j>`, `<C-k>`, `\<`, `\>`, `\\`, `clipboard=unnamedplus`), plus `o`/`O` and other
`task-status-cycler` maps. Confirm a lone `y` still behaves as the normal yank operator.

## Scope, constraints, and sync discipline

- **In scope:** make `ys{count}{motion}{char}` work for the v1 delimiter set, fixing only the trigger/dispatch mechanism
  (and simplifying the speculative range handling once the real shape is known). Out of scope (unchanged from the
  original plan): `ds`/`cs`, visual-mode `S`, HTML/tag surround, wiki-link/markdown wrappers, aliases, dot-repeat.
- **Files touched:** `~/bob/.obsidian/plugins/bob-vim-surround/main.js` (and `manifest.json` only if needed). No
  `obsidian_vimrc.md`, `supportJsCommands`, or `bob-cli` Rust changes.
- **Vault git discipline (carried from the original plan):** the `~/bob` working tree carries unrelated dirty/untracked
  files owned by the nightly job. Stage **only** this task's file(s); never `git add -A` in the vault. Commit via the
  `sase_git_commit` workflow. The `.gitignore` allowlist already covers `.obsidian/**/*.js`/`*.json`.
- **No off-the-shelf substitute:** the requirement is arbitrary count + motion (`ys3w`), which rules out More Vim's
  hardcoded targets and `obsidian-vimrc-support`'s `:surround`; an owned plugin remains the right path.

## Implementation steps

1. **Spike (blocking):** in desktop Obsidian dev console, confirm `window.CodeMirrorAdapter.Vim.handleKey` injection of
   the non-conflicting trigger fires a custom operator, and capture the exact motion-span shape the operator receives.
   Decide primary vs fallback from the result.
2. **Rework the trigger/dispatch** in `main.js`: register the operator under the non-conflicting trigger; add the
   capture-phase `y`→`s` interception that clears the dangling yank and injects the trigger; let the motion flow.
3. **Wire the span source** to whatever the spike proved correct (operator `ranges` with the real shape, or a
   `v{motion}` + read-selection path); remove the speculative range-guessing code.
4. **Retain/adjust wrap semantics** (trim, padding, single-undo, cursor) and the trailing-char capture + cancel
   behavior.
5. **Verify** via the dev-console spike + manual checklist in desktop Obsidian (or via a user-run spike+checklist if the
   agent cannot drive Obsidian). Do not accept a Node-only simulation as proof of the key path.
6. **Commit** only the touched vault file(s) via `sase_git_commit`, leaving unrelated vault dirt untouched.
