---
create_time: 2026-06-26 10:10:45
status: wip
prompt: sdd/prompts/202606/project_status_badges_child_picker.md
---
# Plan: Project status badges in the `<ctrl+=>` child-note picker

## Summary

The `<ctrl+=>` hotkey runs the **Open child note** command from the `bob-navigation-hotkeys` Obsidian plugin. It lists
every note whose `parent` frontmatter property points at the current note and lets the user fuzzy-filter and open one.
Today every child renders identically — a muted file icon, the basename, and the path — so there is no way to tell, at a
glance, which children are **project notes** or what state each project is in.

This feature makes project children stand out and surfaces their status (`wip` / `done` / `canceled`) with a colored
icon + emoji status pill, while leaving non-project children and the picker's existing behavior untouched.

## Background: how the feature works today

- Command `open-child-note` → `openChildNotePicker()` → `collectChildNotes(file)` returns every markdown file whose
  `parent` frontmatter link resolves to the active note (sorted alphabetically by path). With 0 children it shows a
  notice; with exactly 1 it opens directly; otherwise it opens `ChildNotePickerModal`.
- `ChildNotePickerModal extends FilteredPickerModal`. Each row is rendered by a `renderItem(file, rowEl, query)`
  callback that draws: a `file-text` icon, the highlighted basename, and the highlighted path. `FilteredPickerModal`
  already supports an optional right-aligned status element (the `bob-cnp-row-status` pill) — it is used by the sibling
  link-target and yank-path pickers — so the visual slot and CSS scaffolding for a status badge already exist.

## Data model (verified against the vault)

- **Project marker (canonical):** a note is a project iff its frontmatter `type` contains the wikilink `"[[project]]"`
  (as a string or inside an array). This is exactly the rule the `bob-project-tasks` plugin uses (`isProjectType` /
  `PROJECT_TYPE = "[[project]]"`). Reusing it keeps a single, consistent definition of "project" across the plugin
  suite.
- **Status:** project notes carry a `status` frontmatter scalar. In the vault today canonical projects use `wip` (12)
  and `done` (10); the new-project template defaults to `status: wip`. The user wants `canceled` supported as a
  first-class status as well.
- **Critical constraint — status is NOT project-exclusive.** The `status` field is used vault-wide by other note types:
  `wip` also appears on 14 `[[ref]]` notes and 7 book notes; other types use `read` / `liked` / `legacy` / `abandoned` /
  `active`. Therefore **status badges must be gated on the project `type` marker** — never inferred from the presence of
  a `status` value alone, or refs and books would be mislabeled as projects.
- **Legacy zorg projects (intentionally excluded).** ~21 zorg-generated `prj_*.md` notes are conceptually projects but
  were never given `type: "[[project]]"` (19 have no `status`; 2 are `canceled`). They are deliberately **not** treated
  as projects here: badging them would scatter ~19 noisy "No status" pills through the picker, and the convention is
  moving to the canonical `type` marker. Adding `type: "[[project]]"` to such a note opts it in.

## Design

Three pillars, and how each is honored:

- **Intuitive** — status is conveyed redundantly (icon shape + icon color + emoji + word), so it reads instantly whether
  the user keys on color, glyph, or text. Only project children are decorated, so the signal stays meaningful.
- **Reliable** — one well-defined project rule (`type` contains `[[project]]`), shared with `bob-project-tasks`. Status
  display is gated on it, so it can never false-positive on refs/books. Existing behaviors (alphabetical order, single
  -child shortcut, empty/notice states, keyboard nav) are unchanged. Unknown or missing statuses degrade gracefully
  instead of breaking.
- **Beautiful** — reuses the picker's existing theme-native pill styling (`bob-cnp-row-status`, built on Obsidian CSS
  variables) so it looks at home in light/dark/community themes. Adds tasteful semantic color (amber / green / red) and
  a single emoji per status rather than a rainbow of decorations.

### Row anatomy

For a **project** child:

```
[ status-colored Lucide icon ]  Project Title          [ 🚧 WIP ]   ↵
                                 path/to/project.md
```

For a **non-project** child: rendered exactly as today (muted `file-text` icon, title, path, no pill).

### Status → presentation map

| `status` value            | row icon (Lucide) | pill text     | accent |
| ------------------------- | ----------------- | ------------- | ------ |
| `wip`                     | `hammer`          | `🚧 WIP`      | amber  |
| `done`                    | `circle-check`    | `✅ Done`     | green  |
| `canceled` / `cancelled`  | `circle-slash`    | `🚫 Canceled` | red    |
| any other non-empty value | `square-kanban`   | `<Value>`     | muted  |
| missing / empty           | `square-kanban`   | `No status`   | muted  |

Notes on the map:

- Emoji are reserved for the three first-class statuses so the picker stays crisp; unrecognized statuses fall back to a
  clean, capitalized text-only pill (this is what makes future statuses — e.g. `paused`, `blocked` — degrade gracefully
  with zero code changes). A project with no `status` shows a muted "No status" pill, which doubles as a gentle nudge to
  set one.
- `canceled` is forward-looking: no canonical project uses it in the vault yet (the template defaults to `wip`, and the
  only `canceled` notes are the excluded legacy `prj_*` ones), but the user listed it explicitly and it will appear the
  moment a canonical project is marked `status: canceled`.
- Status matching is normalized: take the first element if `status` is an array, coerce to string, strip surrounding
  quotes/`[[…]]`, trim, lowercase.
- Icon names are chosen from widely-available Lucide glyphs; `applyIcon` already degrades silently if a name is missing
  in the bundled icon set.

### Supporting touches

- **Subtitle summary.** When the (unfiltered) result set contains projects, extend the header subtitle with a compact
  breakdown, e.g. `8 notes under sase · 5 projects · 3 wip · 1 done · 1 canceled` (zero counts omitted). Falls back to
  the current text when there are no projects, and to the existing "Showing X of Y" while filtering.
- **Filtering.** Include the status label and a `project` keyword in the row filter so typing `wip`, `done`, `canceled`,
  or `project` narrows the list — in addition to the existing basename/path matching.
- **Accessibility.** Give the pill a `title` / `aria-label` such as "Project status: WIP" so the status is conveyed
  beyond color/emoji.
- **Sort order is unchanged** (alphabetical by path). Predictable ordering is part of "reliable," and the new
  icons/pills already make projects scannable without reordering. (A future option — floating `wip` projects to the top
  — is noted but deliberately out of scope here.)

## Implementation outline

All changes are confined to the **`bob-navigation-hotkeys`** plugin in the `bob-plugins` linked repo. No other plugin,
the vault, or note authoring is touched.

1. **`plugins/bob-navigation-hotkeys/main.js`**
   - Add module-level constants: the project type wikilink, the status → {icon, emoji, label, css-variant} configuration
     table, and the canceled spelling aliases.
   - Add small pure helpers: `isProjectType(value)` (string/array, mirrors `bob-project-tasks`),
     `normalizeStatus(value)`, and `getProjectNoteInfo(frontmatter)` returning
     `{ isProject, statusKey, label, emoji, icon, variant }`.
   - In `ChildNotePickerModal`: precompute a per-file project-info lookup once at construction (reading
     `app.metadataCache.getFileCache(file)?.frontmatter`), so per-keystroke `renderItem` does no extra cache work. Then:
     - `renderItem` chooses the status icon (vs the default `file-text`) and appends a `bob-cnp-row-status` pill for
       projects only.
     - `getSubtitle` appends the project/status breakdown.
     - `filterItem` also matches the status label and the `project` keyword.
   - Keep `collectChildNotes`, `openChildNote`, and the single-child shortcut exactly as-is.
2. **`plugins/bob-navigation-hotkeys/styles.css`**
   - Add `bob-cnp-row-status` color variants (`is-status-wip` / `-done` / `-canceled` / `-muted`) using `color-mix` over
     Obsidian's named color variables, matching the existing pill aesthetic.
   - Add matching status-color rules for the row icon, including selected-state precedence so the status color survives
     row selection.
3. **`plugins/bob-navigation-hotkeys/manifest.json`**
   - Bump `version` `1.1.1` → `1.2.0` (additive feature).

## Verification

There is no unit-test harness for these plugins (the repo ships only `scripts/validate-manifests.mjs`, a manifest
check + `node --check` syntax pass). Verify by:

1. Run `npm run validate` in the `bob-plugins` repo (syntax + manifest sanity).
2. Deploy to the vault from the `bob-plugins` checkout: `bob plugins sync -p bob-navigation-hotkeys -r "$PWD" --dry-run`
   then without `--dry-run`, and reload the plugin in Obsidian.
3. Manual test matrix with `<ctrl+=>`:
   - A parent with mixed `wip` + `done` project children (e.g. `sase`) — confirm amber `🚧 WIP` and green `✅ Done`
     pills + matching icons + subtitle counts.
   - A parent whose children are all `done` (e.g. `bob`).
   - A parent with both project and non-project children — confirm only the projects get badges and plain notes are
     visually unchanged.
   - Temporarily set one canonical project to `status: canceled` to confirm the red `🚫 Canceled` rendering, then
     revert.
   - A project note with an unusual/empty `status` — confirm the muted fallback pill.
   - Type `wip` / `done` / `project` in the filter — confirm narrowing works.
   - Confirm legacy `prj_*` children render as plain notes (documented exclusion).

## Risks & decisions for approval

- **Strict canonical project detection** (`type` contains `[[project]]`) rather than a legacy-`prj_*` bridge —
  recommended for reliability and to avoid ~19 noisy "No status" badges. Trade-off: `canceled` isn't exercised by
  existing vault data until a canonical project is marked canceled.
- **Emoji in the pill + Lucide in the icon column** — honors the user's "icon / emoji" request while keeping the badge
  accessible (word + color + `title`) and the icon column theme-native.
- **Status vocabulary** is the three first-class values with a graceful text-only fallback for anything else, so the
  feature won't need code changes as the user's status vocabulary grows.

## Out of scope

- Changing how projects or statuses are authored (frontmatter, the `new_project` template, the `bob-project-tasks`
  plugin).
- Migrating or re-typing legacy zorg `prj_*` notes.
- Re-sorting / grouping the picker, or applying badges to any picker other than the child-note picker.
