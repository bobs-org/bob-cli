---
create_time: 2026-06-18 14:44:55
status: wip
prompt: sdd/prompts/202606/inline_code_styling.md
---
# Plan: Beautiful inline code styling for the Bob Obsidian vault

## Goal

Make inline code (single-backtick spans like `` `foo` ``) in the Bob vault look noticeably more beautiful and polished,
in **both** reading view and live preview, in **both** light and dark appearance, without touching block/fenced code.
The styling must travel via **Obsidian Sync** so it shows up on every device.

I'm leading the design here. The target aesthetic: a crisp, modern "code chip" — a soft theme-adaptive tinted
background, a hairline border that cleanly delineates the span (the single biggest upgrade over Obsidian's borderless
default), gently rounded corners, tight padding, and a whisper of depth. Think GitHub/Linear-grade inline code, tuned to
sit harmoniously inside Bryan's existing vault styling.

## Context / current state (researched)

- **Vault:** `/home/bryan/bob` (git-tracked; `.gitignore` explicitly _includes_ `.obsidian/**/*.css`, so snippets travel
  with the notes). Default Obsidian theme (no custom theme).
- **Snippets:** `~/bob/.obsidian/snippets/` already contains `task-statuses.css` and `dataview-properties.css`, both
  enabled in `appearance.json` → `enabledCssSnippets`.
- **Obsidian Sync:** core `sync` plugin is **enabled**; `.obsidian/sync.json` + `.sync.lock` are gitignored as
  "device-local state." The two existing snippets already round-trip across devices, which is strong evidence that
  Sync's **"themes & snippets"** config-sync category is already ON.
- **House CSS style** (from `dataview-properties.css`): layered fallbacks (plain `rgba`/hex first, then a `color-mix()`
  override), Obsidian CSS variables, `em` units for scale-independence, `box-decoration-break: clone` for wrapped inline
  spans, and a comment header. The new snippet will match this house style for visual + code cohesion.

## How it syncs (the explicit requirement)

CSS snippets are part of the vault's `.obsidian/` config. The change is just **two files inside `~/bob/.obsidian/`**:

1. A new snippet file `~/bob/.obsidian/snippets/inline-code.css` (the CSS itself).
2. Adding `"inline-code"` to `enabledCssSnippets` in `~/bob/.obsidian/appearance.json` (turns it on).

Both ride Obsidian Sync's "themes & snippets" config category — the same path the two existing, already-synced snippets
use. So once Obsidian on this machine picks up the files, Sync pushes them to every other device automatically. (They
are also git-tracked, as a belt-and-suspenders backup.)

**Verification step in the plan:** because the sync category can't be safely toggled from the filesystem (it's
server/plugin state, not a plain file), after applying the change I'll confirm the snippet is enabled and ask Bryan to
confirm it appears on a second device — though the existing synced snippets make this essentially a sanity check, not a
risk.

## Design vision (what "beautiful" means here)

A single, theme-adaptive "code chip" treatment:

- **Background:** a soft tint built with `color-mix()` against `--background-primary` (neutral base with a faint accent
  lean), so it's clearly a distinct chip without shouting — and legible on top of callout/table backgrounds too.
- **Border:** a 1px hairline from a low-opacity blend of the accent and the theme's border color. This crisp outline is
  the headline upgrade vs. the default borderless look.
- **Corners:** `border-radius` ≈ `0.35em` — matches the radius language of `dataview-properties.css` for cross-snippet
  cohesion.
- **Padding:** tight — roughly `0.15em` vertical / `0.4em` horizontal, so it hugs the text.
- **Type:** `--font-monospace`, ~`0.875em`, ligatures disabled for crisp code; text color stays high-contrast
  (`--code-normal`/`--text-normal`) with at most a very subtle accent lean so it reads cleanly inside headings,
  callouts, and tables.
- **Depth:** a whisper of shadow (`0 1px 0 rgba(0,0,0,0.03)`), echoing the dataview chips.
- **Wrapping:** `box-decoration-break: clone` + `overflow-wrap`, so a chip that wraps across lines keeps its rounded,
  bordered shape on every line.
- **Scale-independent:** `em` units mean the chip scales gracefully inside larger headings.

It will be tuned to look great in **both** light and dark mode via `color-mix()` against Obsidian's theme variables,
with plain-color fallbacks declared first (progressive enhancement, matching the existing snippet).

### Indicative sketch (design direction; exact values refined during implementation)

```css
body {
  --inline-code-radius: 0.35em;
  --inline-code-bg: color-mix(in srgb, var(--interactive-accent) 8%, var(--background-primary));
  --inline-code-border: color-mix(in srgb, var(--interactive-accent) 22%, var(--background-modifier-border));
  --inline-code-color: var(--code-normal, var(--text-normal));
}
/* Reading view: a single clean element — gets the full chip treatment. */
:not(pre) > code {
  padding: 0.15em 0.4em;
  border: 1px solid var(--inline-code-border);
  border-radius: var(--inline-code-radius);
  background: var(--inline-code-bg);
  color: var(--inline-code-color);
  font-size: 0.875em;
  box-shadow: 0 1px 0 rgba(0, 0, 0, 0.03);
  box-decoration-break: clone;
}
/* Live Preview / Source (CodeMirror): continuous tinted pill across the span. */
.cm-inline-code {
  /* background + vertical padding + rounded ends; dim backticks */
}
```

## Technical approach (two render contexts)

Obsidian renders inline code in two different DOMs, so the snippet targets both:

1. **Reading view** — inline code is a bare `<code>` element (block code is `<pre><code>`). Target `:not(pre) > code` so
   fenced blocks are untouched. This is a single element and gets the full chip treatment (bg + border + radius +
   shadow + padding).
2. **Live Preview & Source mode** — CodeMirror 6 renders inline code as `.cm-inline-code` spans, where the backtick
   markers are separate `.cm-formatting-code` spans. Because per-span borders fragment visually, here I'll use a
   **continuous tinted background** + vertical padding + `box-decoration-break: clone`, round only the outer ends, and
   dim the backtick glyphs slightly so the run reads as one cohesive pill — visually consistent with reading view rather
   than identical.

**Explicitly out of scope of the selectors (won't be touched):** fenced/block code (`pre > code`, `.HyperMD-codeblock`,
`.cm-hmd-codeblock`), so multi-line code blocks keep their current look.

## Files to change

| File                                       | Change                                                     |
| ------------------------------------------ | ---------------------------------------------------------- |
| `~/bob/.obsidian/snippets/inline-code.css` | **New** — the inline-code chip snippet (house style).      |
| `~/bob/.obsidian/appearance.json`          | **Edit** — append `"inline-code"` to `enabledCssSnippets`. |

No changes inside the `bob-cli` workspace itself; the deliverable lives in the vault.

## Validation

- Add a scratch note with inline code in: a paragraph, a heading, a list item, a table cell, and a callout — plus a
  fenced code block — to confirm chips render everywhere _and_ that the fenced block is unaffected.
- Eyeball it in all four combinations: {light, dark} × {reading view, live preview}.
- Confirm `appearance.json` lists `inline-code` and the snippet is active.
- Sanity-check sync: confirm the snippet appears/enabled on a second device (low risk given existing snippets already
  sync).

## Risks & mitigations

- **Sync category off:** unlikely (existing snippets sync). Mitigation: verification step above; if off, Bryan flips
  Settings → Sync → "themes & snippets" (one toggle, can't be done from disk safely).
- **Editor pill fragmentation:** inherent to CodeMirror's span model. Mitigation: continuous background + clone + dimmed
  backticks instead of fragmenting borders.
- **Contrast in callouts/dark mode:** mitigated by `color-mix()` against theme variables + fallbacks, and verified
  across all four mode combinations.

## Out of scope

- Restyling fenced/block code, syntax highlighting, or code-block "copy" buttons.
- Installing a community theme or any plugin.
- Changing the two existing snippets.
