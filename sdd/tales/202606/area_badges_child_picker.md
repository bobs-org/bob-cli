---
create_time: 2026-06-27 07:39:43
status: wip
prompt: sdd/prompts/202606/area_badges_child_picker.md
---
# Plan: Area badges in the `<ctrl+=>` child-note picker

## Summary

The `<ctrl+=>` hotkey runs the **Open child note** command from the `bob-navigation-hotkeys` Obsidian plugin. It lists
every note whose `parent` frontmatter points at the current note and lets the user fuzzy-filter and open one. We
recently taught the picker to badge **project** children (`type: [[project]]`) with a status-colored icon + emoji pill.

This feature does the analogous thing for **area** children (`type: [[area]]`) — but areas are a fundamentally different
kind of note, so the design treats them differently rather than copy-pasting the project pattern. Projects are
_stateful_ (they have a `wip`/`done`/`canceled` lifecycle, so they get a status-varying badge). Areas are _ongoing
spheres of responsibility_ with **no status**, so each area gets a single, uniform, beautiful "Area" badge — a compass
icon and a `🧭 Area` pill in a distinct color that reads as a _category_, not a _state_.

The result: in a mixed child list (e.g. under `gtd`), a project shows `🚧 WIP`, an area shows `🧭 Area`, and a plain
note stays plain — three instantly distinguishable kinds, all theme-native.

## Background: how the picker works today (post project-badges)

- `open-child-note` → `openChildNotePicker()` → `collectChildNotes(file)` returns every markdown file whose `parent`
  frontmatter link resolves to the active note (sorted by path). 0 children → notice; exactly 1 → open directly;
  otherwise → `ChildNotePickerModal`.
- `ChildNotePickerModal extends FilteredPickerModal`. At construction it precomputes a `projectInfoByPath` map once
  (reading `app.metadataCache.getFileCache(file)?.frontmatter`); `renderItem` / `getSubtitle` / `filterItem` then only
  consult that map, so per-keystroke rendering does no cache work.
- A project row draws a status-colored Lucide icon (instead of the muted `file-text`) plus a right-aligned
  `bob-cnp-row-status` pill (`emoji + label`). The icon and pill both carry an `is-status-<variant>` class; CSS colors
  them with `color-mix` over Obsidian's named color variables, with selected-state precedence so the color survives row
  selection. The pill slot, highlight helper, and icon helper (`applyIcon`) are all already in place.
- The file already knows "area" exists conceptually: `PROJECT_PARENT_TYPE_BASENAMES = new Set(["area", "project"])`.

## Data model (verified against the vault)

- **Area marker (canonical):** a note is an area iff its frontmatter `type` contains the wikilink `"[[area]]"` (string
  or array member) — exactly mirroring the project rule. There are **10** area notes today:
  `body, cash, dev, gtd, gtd_daily, inbox, job, love, mac_inbox, recur`.
- **Areas are status-less.** 9 of 10 carry no `status` field at all; the lone exception (`inbox`) has a stray
  `status: active`. There is **no** active/archived convention for areas in the vault — it simply does not exist in the
  data. This is the decisive difference from projects and the reason areas get one uniform badge, not a status-varying
  one.
- **Areas appear as children.** All 10 areas have a `parent`, so all are eligible to show in the picker. Four nest under
  real (non-`[[area]]`) parents: `gtd_daily`→`gtd`, `inbox`→`gtd`, `mac_inbox`→`inbox`, `recur`→`cash`.
- **Area and project are mutually exclusive** — 0 notes carry both `[[area]]` and `[[project]]` in `type`. This lets a
  single classifier resolve a note to exactly one kind with simple precedence, no ambiguity.
- **Full `type` taxonomy** (for context): `day` 1085, `ref` 330, `restaurant` 91, `project` 24, `area` 10, `done` 8 —
  these six are the only `type` values in the vault. Only `project` and `area` are organizational containers that make
  sense to badge in this picker; the rest are leaf/reference notes that rarely (if ever) appear as `parent`-linked
  children and are intentionally left plain.

## Design

Three pillars, and how each is honored:

- **Intuitive** — an area reads as a _different kind of thing_, not another project status. It gets its own glyph
  (compass = an ongoing bearing/area of responsibility), its own emoji (🧭), and its own color, conveyed redundantly
  (icon shape + icon color + emoji + the word "Area") so it lands whether the user keys on color, glyph, or text.
- **Reliable** — one well-defined area rule (`type` contains `[[area]]`), parallel to the project rule and provably
  disjoint from it. Areas deliberately ignore any stray `status` (e.g. `inbox`'s `active`) so every area badge is
  identical — no accidental, inconsistent "active" pill on one area and nothing on the others. Existing behavior
  (alphabetical order, single-child shortcut, empty/notice states, keyboard nav) and **all existing project rendering**
  are unchanged.
- **Beautiful** — reuses the picker's existing theme-native pill/icon styling, adds exactly one tasteful new accent
  (purple) and one emoji, and keeps the visual language consistent with projects (colored Lucide icon on the left,
  `emoji + word` pill on the right). Consistency _is_ the beauty here: a third kind slots into an established grammar.

### Row anatomy

For an **area** child:

```
[ purple compass icon ]  Area Title              [ 🧭 Area ]   ↵
                         path/to/area.md
```

For a **project** child: unchanged (status-colored icon + `🚧 WIP` / `✅ Done` / `🚫 Canceled` / fallback pill). For a
**plain** child: unchanged (muted `file-text` icon, title, path, no pill).

### Area → presentation

| `type` contains | row icon (Lucide) | pill text | accent | aria/title  |
| --------------- | ----------------- | --------- | ------ | ----------- |
| `[[area]]`      | `compass`         | `🧭 Area` | purple | `Area note` |

Notes on the choice:

- **One badge, no status axis.** Areas are ongoing; inventing an active/archived status would fabricate a convention the
  vault doesn't have. If a future area-status convention ever emerges, it can be layered on later — out of scope now.
- **Color = purple** (`--color-purple`). It is maximally separated from the project status palette (amber / green / red)
  and the muted-gray fallback, and purple conventionally signals a _category/label_ rather than a _state_ — exactly the
  project-vs-area distinction we want to telegraph. (Easy swap to `--color-blue` if preferred at review — see Risks.)
- **Compass + 🧭** mirror the project pattern of a matched icon/emoji metaphor (e.g. `hammer` + 🚧). Both are core
  Lucide / standard-emoji glyphs; `applyIcon` already degrades silently if an icon name is ever missing.
- **Stray status ignored on purpose.** `inbox`'s `status: active` does not alter its badge — every area renders
  identically. Documented decision, not an oversight.

### Architecture: a small, low-risk generalization (not a copy-paste)

Because project and area are mutually exclusive, the clean design is to introduce **one unified "child-note decoration"
resolver** that the modal consumes, layered _on top of_ the existing, untouched project logic:

- Keep `isProjectType`, `normalizeStatus`, `getProjectNoteInfo`, and the entire `PROJECT_STATUS_*` configuration exactly
  as they are — **zero changes to project detection or status rendering**, so the just-shipped project feature cannot
  regress.
- Add the area equivalents: an `AREA_TYPE_WIKILINK` constant, a single frozen `AREA_PRESENTATION`
  (`{ icon: "compass", emoji: "🧭", label: "Area", variant: "area" }`), and `isAreaType(value)` (mirrors
  `isProjectType`, string-or-array).
- Add a thin combiner `getChildNoteInfo(frontmatter)` returning a **common shape** used by the modal:
  `{ kind: "project" | "area" | "plain", decorated, icon, emoji, label, variant, statusKey }`. It returns the project
  decoration first (derived verbatim from the unchanged `getProjectNoteInfo`), else the area decoration, else the plain
  default (`icon: "file-text"`). Project precedence is explicit (and moot given disjointness).
- The modal's precomputed map becomes `noteInfoByPath` (carrying this common shape). `renderItem` keys on
  `info.decorated` and reads `info.icon` / `info.variant` / `info.emoji` / `info.label` — values identical to today for
  projects, so project rows render pixel-for-pixel the same; area rows light up via the same code path.

This keeps one render branch, one pill component, one CSS family — the picker stays small and coherent instead of
growing a parallel area code path.

### Supporting touches

- **Subtitle summary.** Extend the existing project breakdown to also count areas. Examples:
  - under `gtd` (1 project `wip`, 2 areas, rest plain): `9 notes under gtd · 1 project · 1 wip · 2 areas`
  - under `area` (6 area children): `6 notes under area · 6 areas`
  - no projects/areas: unchanged current text. While filtering: unchanged `Showing X of Y`. Area count is a peer phrase
    appended after the project block; pluralized (`1 area` / `N areas`); omitted when zero.
- **Filtering.** Add an `area` keyword (and the "Area" label) to area rows' searchable text so typing `area` narrows to
  area children, just as `project` / `wip` / `done` already narrow projects. Plain notes still match basename/path only.
- **Accessibility.** The area pill gets `title` / `aria-label` = `Area note`, parallel to the project pill's
  `Project status: …`, so the kind is conveyed beyond color/emoji.
- **Sort order unchanged** (alphabetical by path). The icon/pill make areas scannable without reordering.

## Implementation outline

All changes are confined to the **`bob-navigation-hotkeys`** plugin in the `bob-plugins` linked repo. No other plugin,
the vault, or any note authoring is touched.

1. **`plugins/bob-navigation-hotkeys/main.js`**
   - Add module-level constants: `AREA_TYPE_WIKILINK = "[[area]]"` and a frozen `AREA_PRESENTATION`
     (`icon`/`emoji`/`label`/`variant`), placed beside the existing `PROJECT_*` constants.
   - Add `isAreaType(value)` (string/array, mirroring `isProjectType`).
   - Add `getChildNoteInfo(frontmatter)` (the unified resolver above) and point the modal's per-file lookup at it
     (renaming `getFileProjectNoteInfo` → `getFileChildNoteInfo` / the map → `noteInfoByPath`). Leave
     `getProjectNoteInfo` and all project status logic untouched.
   - Generalize the three modal consumers to the common shape:
     - `renderItem`: branch on `info.decorated` (was `info.isProject`); icon/pill read `info.icon`/`variant`/`emoji`/
       `label`. Area pill `aria-label`/`title` = `Area note`.
     - subtitle helper (`getChildProjectStatusSummary` → a `getChildNoteSummary`): append `N area(s)` after the project
       parts.
     - search helper (`getChildNoteSearchText`): add `area` + `Area` terms for area rows.
   - Keep `collectChildNotes`, `openChildNote`, `openChildNotePicker`, and the single-child shortcut exactly as-is.
2. **`plugins/bob-navigation-hotkeys/styles.css`**
   - Add `.bob-cnp-row-status.is-status-area` (purple pill via `color-mix` over `--color-purple`, matching the existing
     variant recipe) and the matching `.bob-cnp-row-icon.is-status-area` rule, including the `.is-selected` precedence
     selector so the area color survives row selection. (Reuses the existing `is-status-<variant>` class family; the
     variant token is `area`.)
3. **`plugins/bob-navigation-hotkeys/manifest.json`**
   - Bump `version` `1.2.0` → `1.3.0` (additive feature).

## Verification

There is no unit-test harness for these plugins (the repo ships `scripts/validate-manifests.mjs` — a manifest check +
`node --check` syntax pass). Verify by:

1. `npm run validate` in the `bob-plugins` repo (syntax + manifest sanity); `node --check` the plugin;
   `git diff --check`.
2. Deploy from the `bob-plugins` checkout: `bob plugins sync -p bob-navigation-hotkeys -r "$PWD" --dry-run`, then
   without `--dry-run`; reload the plugin in Obsidian.
3. Manual test matrix with `<ctrl+=>`:
   - Parent `gtd` (mixed: project + 2 areas + plain) — confirm the project keeps its `🚧 WIP` badge, both areas show the
     purple `🧭 Area` badge + compass icon, plain notes are visually unchanged, and the subtitle reads
     `9 notes under gtd · 1 project · 1 wip · 2 areas`.
   - Parent `area` (6 area children) — confirm all six render the area badge and the subtitle reads
     `6 notes under area · 6 areas`.
   - Confirm `inbox` (an area with stray `status: active`) renders the identical uniform `🧭 Area` badge (status
     ignored).
   - A project-only parent and a plain-only parent — confirm **no** behavior change (project regression check + plain
     unchanged).
   - Type `area` in the filter — confirm it narrows to area children; type `project` / `wip` — confirm projects still
     narrow (regression).

## Risks & decisions for approval

- **Uniform area badge with no status axis** (areas ignore any `status`) — recommended, because areas have no status
  convention in the vault and surfacing one note's stray `active` would be inconsistent. Trade-off: if you later
  introduce an area lifecycle, the badge will need a follow-up to vary by it.
- **Accent color = purple** vs. blue. Purple maximizes separation from the project status palette and reads as a
  "category." Trivial to switch to `--color-blue` (one variant block) if you prefer a calmer, more structural feel —
  flag it at review.
- **Unified resolver vs. parallel area path.** Generalizing keeps one render/pill/CSS path and is provably safe given
  project/area disjointness; it does lightly touch the modal wiring (not the project status logic). The plan pins
  project rendering as a regression check to guarantee no visible change.

## Out of scope

- Changing how areas (or projects) are authored (frontmatter, templates, the `bob-project-tasks` plugin).
- Inventing an area active/archived status, or badging any non-`project`/`area` `type` (`ref`, `book`, `restaurant`,
  `day`, `done`).
- Re-sorting / grouping the picker, or applying badges to any picker other than the child-note picker.
