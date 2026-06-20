---
create_time: 2026-06-20 09:22:33
status: done
prompt: sdd/prompts/202606/fix_vim_surround_keymaps.md
---
# Plan: Fix all broken `bob-vim-surround` keymaps (`ys`, `cs`, `ds`)

## Summary

All three `bob-vim-surround` operators (`ys`, `cs`, `ds`) are dead in Obsidian. `ys` worked after last night's fix; the
`cs` commit (`3bafa34`) introduced a **physical-key "trigger candidate" dispatch** that is silently clobbered by a
pre-existing **double-registered capture-phase keydown listener**. The bug was latent before `cs` and is now fatal to
every operator. This is a **one-line, root-cause fix** in the vault plugin — not a rewrite. It has been **reproduced and
verified headlessly** before writing this plan.

This is a **vault-side change** to `~/bob` (`.obsidian/plugins/bob-vim-surround/main.js`), not a `bob-cli` (Rust)
change. Same domain, constraints, and git discipline as the prior surround plans.

## Root cause (diagnosed + empirically reproduced)

`main.js` registers its capture-phase `keydown` handler on **both `window` and `document`**
(`registerSurroundInputListeners`, unchanged since the original commit `6ec63a7`):

```js
if (windowObject) targets.push(windowObject);
if (documentObject && documentObject !== windowObject) targets.push(documentObject);
for (const target of targets) target.addEventListener("keydown", keydownHandler, true);
```

For one physical keypress, capture order is **window → document**, so the same handler runs **twice** on the **same
event object**.

In the `ys`-only version this was harmless: `handleSurroundKeydown` began with
`if (!this.pendingSurround) return false;`, so the listener was inert except for the final delimiter key — which it
**consumed** (`stopPropagation`), preventing the document pass from ever running, and which it added to
`handledSurroundEvents`.

The `cs` commit (`3bafa34`) deleted that early-return and added `handlePhysicalSurroundTriggerKeydown`, which on a bare
`y`/`c`/`d`:

1. records `this.surroundTriggerCandidate = { cm, op }`, and
2. **returns `false` without consuming** — deliberately, so Vim still sees the operator key.

Because that path does **not** consume the event, propagation is **not** stopped, so the **document pass re-runs on the
same event**. On that second pass the candidate is now non-null and the key (`y`/`c`/`d`) is not `s`, so this branch
fires and **immediately wipes the candidate**:

```js
if (candidate && key !== "s") {
  if (!isModifierOnlyKey(event)) this.surroundTriggerCandidate = null; // <-- clobbers it
  return false;
}
```

Net effect: the candidate set by the window pass is destroyed by the document pass of the **same** keypress. When `s`
arrives there is no candidate, so `ys`/`cs`/`ds` never dispatch. All three operators are equally dead — matching the
report.

### Reproduction (done, headless)

A throwaway Node harness loaded the real `main.js` (stubbed `obsidian`), registered the listener on stub
`window`+`document`, and dispatched events through the capture path honoring `stopPropagation`:

- Pressing `y` → `surroundTriggerCandidate` is **`null`** immediately afterward (set by window pass, wiped by document
  pass). Pressing `s` → **zero** Vim keys injected, no pending state. `ys` does not fire. The same mechanism kills `cs`
  and `ds`.

## The fix

Make `handleSurroundKeydown` **idempotent per physical event**: mark every event as seen the first time the handler
runs, so the redundant (document) capture pass short-circuits and can never re-run dispatch or clobber candidate state.
One insertion, right after the existing `has()` guard at the top of `handleSurroundKeydown`:

```js
if (this.handledSurroundEvents && this.handledSurroundEvents.has(event)) {
  return false;
}
// NEW: claim this event so a second capture pass (window+document) is a no-op.
if (this.handledSurroundEvents) {
  this.handledSurroundEvents.add(event);
}
```

Why this is the right altitude:

- It fixes the **actual root cause** (one physical event processed twice) rather than patching one symptom branch. The
  existing per-path `handledSurroundEvents.add(event)` calls become redundant but harmless.
- It is robust regardless of how many targets the listener is attached to, and it preserves the intended semantics: the
  window pass still records the candidate and lets the key reach Vim; the document pass is simply skipped.

### Alternative considered

Register the keydown listener on a **single** target (`window` only) instead of both. Window capture is already first in
the propagation path, so this also removes the double-fire and is a clean simplification. **Not chosen as primary**
because it is slightly less defensive (any future need to observe a second target would silently reintroduce
double-processing), whereas the per-event dedupe is correct for any listener set. Could be applied additionally as
cleanup, but is not required.

## Verification (carrying the `ys` process lesson — test the dispatch, not just the helpers)

The original `ys` bug shipped because the only test exercised the wrap helper, never the key dispatch. So the gate here
is the **dispatch path under the double-capture condition**, exactly the thing that broke.

1. **Headless dispatch reproduction + fix proof (already run; will be re-run as the gate):** a Node harness that loads
   the real `main.js`, registers the handler on stub `window`+`document`, and fires events through the capture path with
   `stopPropagation` honored. Assert, on the **fixed** file:
   - `ys`: after `y`, candidate is `{op:"y"}` (survives); after `s`, injected Vim keys are exactly
     `["<Esc>","<A-y>","s"]`. **(verified ✓)**
   - `cs`: after `c` then `s`, injects `["<Esc>"]` and sets `pendingChangeSurround`. **(verified ✓)**
   - `ds`: after `d` then `s`, injects `["<Esc>"]` and sets `pendingDeleteSurround`. **(verified ✓)**
   - `dd` and lone `s` fall through cleanly: no injection, no stray candidate/pending state. **(verified ✓)**
   - And the pre-fix file reproduces the failure (candidate `null` after `y`, nothing injected). **(verified ✓)** Plus
     `node --check` and `git -C ~/bob diff --check` on the touched file.

2. **Desktop Obsidian acceptance (the real key path — user-run gate):** the agent cannot drive desktop Obsidian. After
   reloading the plugin, the user confirms the manual checklist below. Do **not** declare success from the Node
   simulation alone.

Manual acceptance checklist (desktop, Vim mode, Markdown editor) — re-verify **all three** operators in one session
since they share the fixed dispatch:

- **`ys`**: `ysiw"`, `ys3w)`, `yy`/`yw`/`y$` (plain yanks unaffected).
- **`cs`**: `cs"'`, `cs([`, `cs)]`; plain `cc`/`cw`/`ciw` unaffected.
- **`ds`**: `ds"`, `ds'`, `` ds` ``, `ds)`/`ds(` on `(Hello)` and `( Hello )`, nested `([Hello])` `ds)` → `[Hello]`;
  `dd`/`dw`/`diw`/`di(`/`da"`/`d$`/`D`/`"add` unaffected; lone `s` substitutes.
- `<Esc>` mid-operator (after `cs`/`ds` awaiting a target) cancels cleanly. Existing vimrc maps (`[[`, `]]`, `-`, `!`,
  `<C-j>`, `<C-k>`, `o`/`O`) unaffected.

## Files touched

- `~/bob/.obsidian/plugins/bob-vim-surround/main.js` — insert the per-event dedupe (`handledSurroundEvents.add`) at the
  top of `handleSurroundKeydown`, right after the existing `has()` early-return.

No `manifest.json` change required (behavior fix, not a feature; bump to `1.2.1` only if desired). No
`obsidian_vimrc.md`, no `supportJsCommands`, no `bob-cli` Rust changes.

## Implementation steps

1. Insert the `handledSurroundEvents.add(event)` dedupe at the top of `handleSurroundKeydown` (after the existing
   `has()` guard).
2. Run the headless dispatch harness (Verification step 1) confirming all four cases pass on the fixed file and the
   failure reproduces on the unfixed file; run `node --check` and `git -C ~/bob diff --check`.
3. Hand the user the manual checklist (Verification step 2) and treat their confirmation as the acceptance gate;
   re-verify `ys`/`cs`/`ds` together. Do not accept the Node simulation as proof of the real key path.
4. Commit only the touched vault file via the `sase_git_commit` workflow, leaving unrelated vault dirt untouched (never
   `git add -A` in `~/bob`).

## Scope

**In scope:** the single dispatch-dedupe fix that restores `ys`, `cs`, and `ds`, plus the headless dispatch verification
and the manual acceptance checklist.

**Out of scope (unchanged from prior plans):** visual-mode mappings, HTML/tag targets, wiki-link / emphasis wrappers and
aliases, counts, dot-repeat, and any feature additions. This plan only restores the existing, already-implemented
behavior.
