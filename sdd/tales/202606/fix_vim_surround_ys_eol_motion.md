---
create_time: 2026-06-20 09:54:43
status: wip
prompt: sdd/prompts/202606/fix_vim_surround_ys_eol_motion.md
---
# Plan: Fix `ys` with end-of-line motions (`$`) leaving text highlighted instead of surrounding

## Summary

After the prior fix, `bob-vim-surround`'s `ys` works for word/text-object motions (`iw`, `2w`, `3w`, `w`, `e`) but **`$`
(and its alias `<End>`) just highlights the cursor-to-end-of-line text and never surrounds it.** Root cause is a single,
narrow data bug: CodeMirror's Vim encodes the end-of-line column as the sentinel **`ch: Infinity`**, and the plugin's
`normalizePosition` rejects any non-finite `ch`, so the operator range is dropped and the operator bails out **without
collapsing the selection CodeMirror already painted** — leaving the highlight stuck. This is a **one-spot, root-cause
fix** in the vault plugin, **reproduced and verified against the real CodeMirror 6 Vim engine
(`@replit/codemirror-vim@6.3.0`, the same engine Obsidian bundles)** before writing this plan — not a stub.

This is a **vault-side change** to `~/bob` (`.obsidian/plugins/bob-vim-surround/main.js`), not a `bob-cli` (Rust)
change. Same domain, constraints, and git discipline as the prior surround plans.

## Root cause (diagnosed + empirically reproduced against the real engine)

`ys` is implemented by injecting a CodeMirror Vim **operator** (`bobVimSurroundAdd`, mapped to the private `<A-b>s`
bridge). After `ys`, the user's motion drives CodeMirror's operator+motion dispatch (`evalInput`), which:

1. computes the motion's range,
2. **paints a selection via `cm.setSelections(cmSel.ranges)` immediately before invoking the operator**, and
3. calls our operator `handleSurroundOperator(cm, operatorArgs, ranges, oldAnchor, newHead)`, then **only collapses that
   selection if the operator returns a truthy cursor** (`if (operatorMoveTo) cm.setCursor(operatorMoveTo)`).

The `$` motion (`moveToEol`) returns a position of `new Pos(line, Infinity)` — `Infinity` is CodeMirror's "end of line"
sentinel. `makeCmSelection` does **not** clip that sentinel (only `cm.setSelections` clips it when actually rendering
the highlight), so the **range object handed to our operator has `head.ch === Infinity`.**

The plugin's `normalizePosition` guards on finiteness:

```js
function normalizePosition(value) {
  if (!value || !isFiniteNumber(value.line) || !isFiniteNumber(value.ch)) {
    return null;            // Number.isFinite(Infinity) === false  -> range rejected
  }
  ...
}
```

So for `$`: `normalizePosition(head)` → `null` → `normalizeRange` → `null` → `collectSurroundSpans` returns **0 spans**
→ `handleSurroundOperator` does `this.pendingSurround = null; return false;` **without calling `setCursor`.** Net
effect: CodeMirror leaves the full-line selection it just painted **highlighted**, and there is no pending surround, so
the delimiter key (`"`/`)`/…) that follows has nothing to act on. Exactly the reported symptom.

Word and text-object motions return a **finite** `head.ch`, normalize fine, produce a span, and the operator's internal
`setCursor(spans[0].start)` collapses the selection — which is why they work.

### Affected motion class (precise)

Only motions whose result `Pos` carries `ch: Infinity` are affected. In the real engine that is **`moveToEol`**, bound
to **`$`** and (via `keyToKey`) **`<End>`**. `g$` uses `moveToEndOfDisplayLine`, which returns a finite cursor, so it is
**not** affected. `D`/`C`/`Y` also use these motions but as their own built-in operator-motions, not as a motion typed
after `ys`, so they are irrelevant here.

### Reproduction (done, against the REAL CM6 Vim engine — not a stub)

A Node + jsdom harness installed `@replit/codemirror-vim@6.3.0` + `@codemirror/{state,view}`, built a real `EditorView`
with the `vim()` extension, registered the **exact** operator/`mapCommand` the plugin uses, placed the cursor at column
0 of `"hello world foo bar"`, injected the operator trigger, then drove each motion. The operator callback inspected the
real range it received:

| motion | raw `head.ch` from engine | current `normalizePosition`         | result                                      |
| ------ | ------------------------- | ----------------------------------- | ------------------------------------------- |
| `iw`   | `5`                       | valid span                          | works                                       |
| `w`    | `6`                       | valid span                          | works                                       |
| `2w`   | `12`                      | valid span                          | works                                       |
| `e`    | `5`                       | valid span                          | works                                       |
| `$`    | **`Infinity`**            | **null → 0 spans → `return false`** | **highlight stays (`selAfter` = ch 0..19)** |

The harness also confirmed the fix: clipping the `Infinity` endpoint through `cm.clipPos` yields the concrete EOL span
`{line:0,ch:0}..{line:0,ch:19}`, i.e. a valid surround target. (`cm.listSelections()` at operator-call time is likewise
finite `0..19`, an independent confirmation that the only problem is the unclipped `Infinity` in the passed range.)

## The fix

Resolve CodeMirror's end-of-line sentinel **where `cm` is in scope** (the operator-range parsing path), using
CodeMirror's own clamp — the very call `moveToEol` itself uses, `cm.clipPos`, which turns `{line, ch: Infinity}` into
`{line, ch: <eol column>}`.

Minimal, targeted change in `main.js`:

1. Thread `cm` into `normalizeRange` (its **only** caller is `collectSurroundSpans`, which already has `cm`).
2. Add a small helper that resolves a raw operator endpoint: if it has a finite `line` but a **non-finite `ch`** and
   `cm.clipPos` is available, return `normalizePosition(cm.clipPos({ line, ch }))`; otherwise fall back to
   `normalizePosition(raw)` unchanged.
3. Use that helper for the endpoint extraction inside `normalizeRange` (both the array branch and the
   `from/to/anchor/head` branch).

Leave the shared `normalizePosition` untouched so its finite guard still protects every other call site from genuinely
bad data; only the operator-range path gains the EOL-sentinel resolution. Downstream is already safe: once the endpoint
is finite, the existing `trimTrailingWhitespaceFromRange` + `advancePosition` logic recomputes a correct, clamped span.

### Why this altitude

- It fixes the **actual root cause** (an unresolved `Infinity` EOL sentinel), not a per-motion special case — so `$`,
  `<End>`, and any future EOL-sentinel motion are all covered by one change.
- It uses the engine's own semantics (`cm.clipPos`) rather than inventing a clamp, so behavior matches what CodeMirror
  does when it paints the selection.

### Secondary safeguard (recommended, fail-safe)

Given this feature's regression history, also make the **0-span path fail safe**: when `handleSurroundOperator` finds no
spans, return the pre-motion cursor (`oldAnchor`) instead of `false`, so CodeMirror collapses the selection it painted
(`if (operatorMoveTo) cm.setCursor(operatorMoveTo)`) and never leaves a **stuck highlight**, even for some unforeseen
future motion that still yields an unparseable range. This converts the symptom's failure mode from "fail ugly" (stuck
highlight) to "no-op" (cursor unchanged). The primary clip fix already resolves `$`; this is belt-and-suspenders, kept
tightly scoped to the no-span branch.

### Alternatives considered

- **Read `cm.listSelections()` instead of the passed `ranges`.** The live selection is already finite/clipped at
  operator-call time, so this would also work and be robust to any range-shape quirk. **Not chosen** because it is a
  larger behavioral change to the operator (re-deriving spans, re-checking trailing-whitespace semantics) and would
  break the existing stub-based unit tests that call the operator with synthetic ranges and a stub `cm`. The
  `cm.clipPos` clamp is smaller, keeps the current design and tests intact, and targets the exact defect.
- **Loosen `normalizePosition` globally** to accept non-finite `ch`. Rejected: that weakens a shared primitive used for
  cursors and other positions, risking spurious positions elsewhere.

## Verification (carry the lesson: test the dispatch + real engine, not stub helpers)

1. **Real-engine headless harness (the gate the prior stub harness could not provide).** Reuse the
   `@replit/codemirror-vim@6` + jsdom harness. On the **patched** plugin assert:
   - `ys$` produces a span `{line,0}..{line,EOL}` and `applySurround` turns `hello world foo bar` into
     `"hello world foo bar"` (and `(`/`)` etc. correctly); **no residual selection** remains.
   - Regression: `ysiw`, `ysw`, `ys2w`, `yse` still produce correct finite spans and surround correctly.
   - On the **unfixed** file the `$` case still reproduces 0 spans / stuck highlight (proof the change is what fixes
     it).
   - Plus `node --check` and `git -C ~/bob diff --check` on the touched file.
2. **Desktop Obsidian acceptance (the real key path — user-run gate).** The agent cannot drive desktop Obsidian. After
   reloading the plugin, the user confirms the checklist below; do **not** declare success from the Node simulation
   alone.

Manual acceptance checklist (desktop, Vim mode, Markdown editor):

- **`ys` end-of-line (the fix):** `ys$"`, `ys$)`, `` ys$` ``, `ys$'` from the start and middle of a line; `<End>`
  variant `ys<End>"`; on a line with trailing spaces; on an empty line (no-op, **no stuck highlight**).
- **`ys` regression:** `ysiw"`, `ysw)`, `ys2w)`, `ys3w)`, `yse"`; plain `yy`/`yw`/`y$` yanks unaffected.
- **`cs` / `ds` regression:** `cs"'`, `cs)]`, `ds"`, `ds)` on `(Hello)` and `( Hello )`; plain `cc`/`dd`/`diw`
  unaffected.
- `<Esc>` mid-operator cancels cleanly. Existing vimrc maps (`[[`, `]]`, `-`, `!`, `<C-j>`, `<C-k>`, `\<`, `\>`, `\\`)
  unaffected.

## Files touched

- `~/bob/.obsidian/plugins/bob-vim-surround/main.js` — resolve CodeMirror's `ch: Infinity` end-of-line sentinel via
  `cm.clipPos` in the operator-range parsing path (`normalizeRange` + a small helper, threading `cm` from
  `collectSurroundSpans`); optionally make the 0-span operator path return `oldAnchor` so any no-span case collapses the
  selection instead of leaving a highlight.

No `manifest.json` change required (behavior fix, not a feature; bump to `1.2.1` only if desired). No
`obsidian_vimrc.md`, no `supportJsCommands`, no `bob-cli` Rust changes.

## Implementation steps

1. Add the endpoint-resolution helper and thread `cm` into `normalizeRange`; use it for both endpoint branches so a
   non-finite `ch` is clipped via `cm.clipPos` before `normalizePosition`.
2. (Recommended) In `handleSurroundOperator`, return `oldAnchor` on the `spans.length === 0` branch so CodeMirror
   collapses any painted selection.
3. Run the real-engine harness (Verification step 1): confirm `ys$` surrounds and leaves no highlight on the fixed file,
   the failure reproduces on the unfixed file, and `iw`/`w`/`2w`/`e` still pass; run `node --check` and
   `git -C ~/bob diff --check`.
4. Hand the user the manual checklist (Verification step 2) and treat their confirmation as the acceptance gate. Do not
   accept the Node simulation as proof of the real key path.
5. Commit only the touched vault file via the `sase_git_commit` workflow, leaving unrelated vault dirt untouched (never
   `git add -A` in `~/bob`).

## Scope

**In scope:** the single end-of-line-sentinel resolution that makes `ys$` / `ys<End>` surround correctly (plus the
optional no-span fail-safe), the real-engine headless verification, and the manual acceptance checklist.

**Out of scope (unchanged from prior plans):** visual-mode mappings, HTML/tag targets, wiki-link / emphasis wrappers and
aliases, counts on the operator, dot-repeat, and any feature additions. This plan only restores correct `ys` behavior
for end-of-line motions.
