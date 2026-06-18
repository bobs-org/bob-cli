---
create_time: 2026-06-18 15:00:25
status: wip
prompt: sdd/prompts/202606/inline_code_color.md
---
# Plan: Recolor inline-code chips to a distinct, more visible hue

## Goal

The inline-code "chip" snippet I just added looks great structurally, but its **color is too similar to the Dataview
property chips**. Recolor inline code to a hue that is clearly distinct from the Dataview chips (and from the
task-status colors), and make it a touch _more visible_ — while keeping the same chip shape, the light/dark adaptivity,
the reading-view + Live-Preview coverage, and the "fenced/block code untouched" guarantee already in place.

## Root cause (researched, not guessed)

Both snippets derive their entire color identity from the **same** Obsidian variable:

- `~/bob/.obsidian/snippets/dataview-properties.css` → `--dataview-property-accent: var(--interactive-accent, …)`
- `~/bob/.obsidian/snippets/inline-code.css` → `--inline-code-accent: var(--interactive-accent, …)`

So inline code and Dataview properties are **literally the same hue** (whatever Bryan's Obsidian accent is — the default
leans indigo/purple). Their backgrounds and borders are both low-opacity blends of that one accent, which is why a
`` `code span` `` sitting next to a `key:: value` Dataview field reads as "the same thing." Hue separation — not just
opacity tweaks — is the fix.

## The color landscape (what's already "taken")

To pick something genuinely distinct, here's what the vault's existing snippets already claim:

| Element               | Hue today                                              |
| --------------------- | ------------------------------------------------------ |
| Dataview properties   | `--interactive-accent` → **indigo/purple**             |
| Task: in-progress     | `--color-yellow` → **yellow**                          |
| Task: blocked/on-hold | `--color-orange` → **orange**                          |
| Task: cancelled       | `--text-muted` → **gray**                              |
| Inline code (current) | `--interactive-accent` → **indigo/purple** ← the clash |

Free, distinct hues from Obsidian's built-in theme palette (`--color-*`, which adapt automatically across light/dark):
**cyan/teal**, **green**, **blue**, **pink/magenta**, **red**.

- **red** — reads as error/danger; reject.
- **blue** — too adjacent to the indigo accent; weak separation; reject.
- **green** — free and readable, but carries a "success/done" connotation next to task styling; viable but not ideal.
- **pink/magenta** — maximally distinct and very visible, but bold/polarizing for body text.
- **cyan/teal** — far from indigo on the wheel (strong separation from Dataview), unused elsewhere, long-standing "code"
  association in editor themes, and legible in both light and dark.

## Recommendation: teal / cyan (`--color-cyan`)

Switch inline code from the shared `--interactive-accent` to Obsidian's fixed **`--color-cyan`** palette variable. This:

1. **Decouples** inline code from Bryan's accent, so it can never re-converge with Dataview even if the accent changes.
2. Gives **strong hue separation** from the indigo Dataview chips and from the warm task colors.
3. Stays **theme-adaptive** (Obsidian ships a light and a dark `--color-cyan`), matching how `task-statuses.css` already
   uses `--color-yellow`/`--color-orange`.

If the reviewer prefers a different vibe, the same change applies verbatim by swapping one variable — documented
alternates: **pink/magenta** (`--color-pink`, boldest/most visible) or **green** (`--color-green`, softer). Picking the
hue is a one-line decision at review time; everything else in this plan is identical.

## "More visible," not just "different"

Hue alone fixes the clash; to also make it _pop_ a bit more, this plan nudges three levers (current → proposed):

- **Background tint:** `~8%` accent → `~12%` cyan against `--background-primary` (clearly tinted, still calm).
- **Border:** `~24%` accent → `~42%` cyan against `--background-modifier-border` (crisper, more saturated hairline).
- **Text:** currently plain `--code-normal`/`--text-normal` → gently **lean the code text teal** via
  `color-mix(... ~32%, --code-normal)`. A tinted glyph is the single biggest "this is distinct" signal, and at ~32% it
  stays high-contrast in both light (dark-teal on light) and dark (light-teal on dark) modes.
- **Markers** (the dimmed backticks in Live Preview) lean teal too, so the editor pill reads as one cohesive teal run.

## Scope of the change — minimal and low-risk

Only the **color variables** in the `body { … }` block of `inline-code.css` change. All the hard-won structural CSS —
the reading-view `:not(pre) > code` chip, the CodeMirror `.cm-inline-code` continuous-pill logic, the rounded outer
ends, the `box-decoration-break: clone` wrapping, and every `:not(.HyperMD-codeblock)/:not(.cm-hmd-codeblock)`
block-code exclusion — stays **byte-for-byte the same**. No selectors are added or removed; fenced/block code remains
untouched.

`appearance.json` already lists `inline-code` in `enabledCssSnippets`, so **no second file changes** this time.

### Indicative variable block (exact percentages tuned during implementation)

```css
body {
  --inline-code-hue: var(--color-cyan, #00bfbc);
  --inline-code-radius: 0.35em;

  --inline-code-bg: rgba(0, 191, 188, 0.1); /* fallback first (house style) */
  --inline-code-bg: color-mix(in srgb, var(--inline-code-hue) 12%, var(--background-primary, #fff));

  --inline-code-border: rgba(0, 191, 188, 0.42);
  --inline-code-border: color-mix(
    in srgb,
    var(--inline-code-hue) 42%,
    var(--background-modifier-border, rgba(127, 127, 127, 0.2))
  );

  --inline-code-color: var(--code-normal, var(--text-normal, #222));
  --inline-code-color: color-mix(in srgb, var(--inline-code-hue) 32%, var(--code-normal, var(--text-normal, #222)));

  --inline-code-marker-color: color-mix(in srgb, var(--inline-code-hue) 50%, var(--text-muted, #777));
  --inline-code-shadow: 0 1px 0 rgba(0, 0, 0, 0.03);
}
```

(Progressive-enhancement fallbacks declared before each `color-mix()`, matching the existing house style.)

## Files to change

| File                                       | Change                                                                                                                                 |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------- |
| `~/bob/.obsidian/snippets/inline-code.css` | **Edit** — recolor the `body { --inline-code-* }` variables to teal; bump tint/border/text for visibility. Structural rules unchanged. |

No changes inside the `bob-cli` workspace; the deliverable lives in the vault.

## Validation

- Open a scratch note with inline code beside a Dataview `key:: value` field and confirm the two are now **obviously
  different colors** (teal chip vs. indigo property).
- Eyeball all four combinations: {light, dark} × {reading view, live preview} — confirm the teal text stays readable and
  the chip border looks crisp, not muddy.
- Confirm a fenced code block is still completely unaffected.
- No new sync work needed: the snippet file already rides the same Obsidian Sync "themes & snippets" category, and the
  `appearance`/`appearance-data` categories were added last time. Second-device confirmation remains a visual sanity
  check.

## Risks & mitigations

- **Teal text contrast in light mode** (cyan can go pale): mitigated by mixing toward `--code-normal` at ~32% (text
  stays mostly the theme's code color, only _leaning_ teal) and verified across light/dark.
- **Reviewer wants a different hue:** the choice is isolated to a single `--inline-code-hue` variable — swapping to
  `--color-pink`/`--color-green` is a one-line change with no structural impact.
- **Accent re-convergence:** eliminated — moving off `--interactive-accent` to a fixed palette color means inline code
  can never track the Dataview accent again.

## Out of scope

- Touching the Dataview, task-status, or any other snippet.
- Changing inline-code _shape/structure_ (padding geometry, radius, pill logic) — this is a **color-only** revision.
- Restyling fenced/block code, syntax highlighting, or copy buttons.
