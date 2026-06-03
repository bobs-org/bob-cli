---
create_time: 2026-06-03 11:54:19
status: wip
prompt: sdd/prompts/202606/obsidian_child_popup_redesign.md
---
# Obsidian Child Note Picker — Visual Redesign Plan

## Goal

The child-note popup (`Ctrl+Alt+C`) is now correctly sized and functional, but it looks plain — it reuses Obsidian's
generic suggestion list and a bare `<h2>` + text input. The user has asked me to **lead the design** and make it look
**WAY better / beautiful**, while keeping all current behavior.

This is a pure presentation/UX-polish pass on the existing `ChildNotePickerModal`. No `bob-cli` Rust change, no new
command, and the `Ctrl+Alt+C` hotkey and all keyboard semantics stay exactly as they are.

## Design Vision

Transform the popup from "a generic list in a box" into a polished, command-palette-class picker — the visual quality of
Obsidian's own Quick Switcher / Raycast / Linear's command palette. The redesign rests on five pillars:

1. **A real header, not a bare heading.** A leading icon chip, a confident title, and a live subtitle that gives context
   ("12 notes under `<parent>`" / "Showing 3 of 12"). This orients the user instantly and makes the popup feel
   purposeful.

2. **A search field that looks like a search field.** A bordered, rounded input wrapper with a leading search icon and a
   clear focus ring, instead of a naked text box. This is the single biggest "cheap vs. polished" tell.

3. **Rich, scannable result rows.** Each row gets a file icon, the note title as prominent primary text, and the
   vault-relative path as muted secondary text. The **matched portion of the query is highlighted** in both title and
   path — the hallmark of a great fuzzy picker and a real scannability win. The selected row gets an accent-tinted
   background, a left accent bar, and a subtle "↵ to open" affordance, with smooth (reduced-motion-aware) transitions.

4. **A keyboard-hint footer.** A quiet bottom bar with styled `kbd` chips — `↑ ↓` Navigate · `^N ^P` Move · `↵` Open ·
   `esc` Dismiss. This is the detail that makes a picker feel "designed" and also teaches the new `Ctrl+N`/`Ctrl+P`
   bindings.

5. **Theme-native, self-owned styling.** Build entirely on Obsidian CSS variables (`--background-*`, `--text-*`,
   `--interactive-accent`, `--radius-*`, `--size-*`, fonts) so it looks correct in light/dark and any theme, and own the
   class namespace (`bob-cnp-*`) so the result no longer inherits — or fights — generic theme suggestion styles.

Plus two "feels great" mechanics:

- **Selected row auto-scrolls into view** as you navigate (currently it can scroll off-screen in the taller list).
- **A proper empty state** — centered icon + friendly message — instead of a lone gray line.

## What Changes

Two vault files (same as before), no new files beyond the existing `styles.css`:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css`

### JavaScript (`main.js`) — structure & behavior wiring

- **Pass the parent file into the modal** so the header subtitle can name it. `openChildNotePicker()` already has the
  parent `file`; thread it through `new ChildNotePickerModal(app, plugin, children, parentFile)` (defaults safe if
  absent).
- **Rebuild `onOpen()` DOM** into the new structure (all under `bob-cnp-*` classes):
  - header: icon chip + title + live subtitle (count);
  - search wrapper: leading search icon + input;
  - results list (`role="listbox"`);
  - footer: keyboard-hint `kbd` chips.
- **Use Obsidian's `setIcon`** for crisp, theme-consistent Lucide icons (header, search, per-row file icon, selected-row
  enter glyph, empty-state icon). Guard every icon call so a missing `setIcon` (e.g. in the stubbed test harness) simply
  no-ops rather than throwing.
- **Render rows** with title + path, each run through a small **match-highlight helper** that wraps the matched query
  substring in a `bob-cnp-hl` span using safe DOM text nodes (no `innerHTML`). The filter is a case-insensitive
  substring match, so the highlight is exact and accurate.
- **Live subtitle**: update on every filter render — total count when unfiltered, "Showing X of N" when filtering.
- **Auto-scroll selection**: after each render, scroll the selected row into view with
  `scrollIntoView({ block: "nearest" })`, guarded for environments without it.
- **Keep all behavior identical**: filtering semantics, wrap-around selection, `ArrowUp/Down`, `Ctrl+N`/`Ctrl+P`,
  `Enter` to open, click/mousedown handling, the `opening` guard, and Escape-to-close (Obsidian default). Keep the
  `is-selected` class on the selected row.

### CSS (`styles.css`) — the bulk of the beauty

Owned, namespaced styles built on Obsidian variables:

- **Modal shell**: keep the generous sizing (≈ `min(92vw, 960px)` × `min(82vh, 840px)`; mobile media query retained),
  remove default content padding so header/footer can be full-bleed, round the corners, hide overflow.
- **Layout**: flex column — header and search fixed, list flexes and scrolls internally, footer pinned at the bottom.
- **Header**: rounded accent-tinted icon chip, strong title, muted subtitle.
- **Search**: bordered rounded wrapper, leading icon, borderless input inside, accent focus ring on focus-within.
- **Rows**: comfortable padding, rounded, icon + two-line text; single-line title and path with ellipsis (preserved);
  distinct **hover** vs **selected** states; selected = accent-tinted background + left accent bar + visible enter
  glyph; smooth transitions gated behind `prefers-reduced-motion`.
- **Highlight**: `bob-cnp-hl` in accent color, semibold.
- **Footer**: top border, muted text, `kbd` chips styled as small bordered keycaps.
- **Polish**: thin custom scrollbar, centered empty state, sensible `min-width`/`box-sizing`.

## Out of Scope

No changes to child collection, parent frontmatter resolution, sorting, filtering logic, file-opening, alternate-file
tracking, Vim mappings, persisted hotkeys, `hotkeys.json`, the manifest, or any `bob-cli` Rust code.

## Verification

Static:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/styles.css
```

Focused behavior harness (stubbed `obsidian` + `@codemirror/view`, with DOM-helper and `setIcon` stubs):

- ArrowDown / `Ctrl+N` advance; ArrowUp / `Ctrl+P` reverse; both `preventDefault`.
- Enter opens the selected child; wrap-around still works.
- Zero-result filtering renders the empty state and does not throw or open a missing file.
- Filtering a query produces a `bob-cnp-hl` highlight span containing the matched substring.
- The subtitle updates to reflect filtered vs. total counts.
- Navigating triggers `scrollIntoView` on the selected row (spy).
- Missing `setIcon` no-ops (icons are guarded) — render still succeeds.

Manual live-vault acceptance:

- Open a note with several children, press `Ctrl+Alt+C`, and confirm the popup looks markedly more polished: header with
  icon + count, real search field, file icons, highlighted matches, clear selected state, and the keyboard-hint footer.
- Navigate with arrows and `Ctrl+N`/`Ctrl+P`; confirm the selected row stays visible (auto-scroll) and `Enter` opens it.
- Filter, confirm match highlighting and the "Showing X of N" subtitle, and confirm long titles/paths stay single-line
  with ellipsis.
- Sanity-check both light and dark mode.

Before finishing:

```bash
git -C /home/bryan/bob status --short
git status --short
```

Then commit **only** the two task files (`main.js`, `styles.css`) via `/sase_git_commit`, leaving the unrelated dirty
vault paths untouched.

## Risks

- **Theme variation**: building on Obsidian variables and owning the `bob-cnp-*` namespace keeps the look stable across
  themes without globally overriding suggestion styles.
- **`setIcon` availability**: all icon calls are guarded, so headless/test contexts degrade gracefully to text-only.
- **Highlight safety**: highlighting uses DOM text nodes, never `innerHTML`, so arbitrary note titles/paths can't inject
  markup.
- **Very long paths**: still handled by single-line ellipsis — the wider modal just shows more before truncating.
