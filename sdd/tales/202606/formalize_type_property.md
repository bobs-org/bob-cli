---
create_time: 2026-06-11 13:04:16
status: wip
prompt: sdd/prompts/202606/formalize_type_property.md
---
# Plan: Formalize the `type` Frontmatter Property

## Goal

Formalize the `type` frontmatter property across the `~/bob` Obsidian vault so that:

1. Every `type` value is a wikilink to a "type note" (e.g. `type: "[[restaurant]]"`), never a plain string.
2. Every daily (`YYYYMMDD.md`), monthly (`YYYYMM.md`), and yearly (`YYYY.md`) note has `type: "[[day]]"` — including the
   ~1,050 historical periodic notes that have no `type` today.
3. Every type has a corresponding note file whose `parent` is `[[type]]`, with a good description; missing ones are
   created.
4. Metadata Menu presents a useful dropdown (all children of `[[type]]`) when entering a `type` value in Obsidian.

## Context Gathered

### Current `type` values in the vault (raw frontmatter counts)

| Raw value    | Count | Files                                                   | Action                      |
| ------------ | ----- | ------------------------------------------------------- | --------------------------- |
| `"[[ref]]"`  | 306   | `ref/**`, `sase_ref.md`                                 | keep (already a link)       |
| `restaurant` | 91    | `eat/*.md`                                              | → `"[[restaurant]]"`        |
| `daily`      | 10    | `2026/20260528.md`–`2026/20260606.md`                   | → `"[[day]]"`               |
| `"[[done]]"` | 7     | `done/**`                                               | keep                        |
| `"[[day]]"`  | 6     | `2026/20260607.md`–`20260611.md`, `_templates/daily.md` | keep                        |
| `project`    | 2     | `job.md`, `obsidian.md`                                 | → `"[[project]]"`           |
| `monthly`    | 2     | `2026/202606.md`, `_templates/monthly.md`               | → `"[[day]]"` (per request) |
| `inbox`      | 2     | `inbox.md`, `gkeep_gdocs_inbox_dump.md`                 | → `"[[inbox]]"`             |
| `yearly`     | 1     | `_templates/yearly.md`                                  | → `"[[day]]"` (per request) |

### Periodic notes needing `type: "[[day]]"` backfill

- 1,034 daily notes matching `**/YYYYMMDD.md` (in `2023/`–`2026/`), 29 monthly notes matching `YYYYMM.md` (vault root +
  `2026/`), 3 yearly notes (`2024.md`, `2025.md`, `2026.md`). Only 16 of these already have `type`, so ~1,050 files need
  a `type: "[[day]]"` line inserted.
- All sampled periodic notes already have a YAML frontmatter block (most are zorg-generated), so the migration is
  "insert one key into existing frontmatter"; the script must still handle a missing frontmatter block defensively.

### Existing type notes

- `type.md` exists ("Types of [[bob]] notes... See this note's children for all types"), `parent: "[[org]]"`.
- `day.md` exists, `parent: "[[type]]"` ✓. Body only mentions daily notes — needs updating since monthly/yearly notes
  will now also use it.
- `ref.md` exists, `parent: "[[type]]"` ✓.
- `done.md` exists but `parent: "[[org]]"` — must become `[[type]]` so it shows up as a child of `[[type]]`.
- `inbox.md` exists but has no `parent` — must gain `parent: "[[type]]"`.
- `restaurant.md` does NOT exist — must be created.
- `project.md` does NOT exist — must be created (`prj.md` and `projects.md` exist but are GTD horizon/index notes, not
  type definitions; the new note will link to them).

### Consumers and producers of `type`

- `eat.base` filters with `note.type == "restaurant"` — breaks when the value becomes a link; must be updated.
- `refs.base` filters by path, not `type` — unaffected.
- No other `.base` files, dataview blocks, or `_meta`/`_generated` dashboards consume `type`.
- bob-cli's `collect_done.rs` already writes link-style `type: "[[done]]"` (`ARCHIVE_TYPE_LINE`) — no code change.
- bob-cli has no daily/monthly/yearly `type` handling; periodic notes get `type` from the Templater templates in
  `_templates/`, so updating `_templates/monthly.md` and `_templates/yearly.md` fixes all future notes.

### Metadata Menu state

- Plugin installed at `~/bob/.obsidian/plugins/metadata-menu/`, v0.8.12, settingsVersion 5, default config:
  `presetFields: []`, no `classFilesPath`, `isAutosuggestEnabled: true`.
- The vault `.gitignore` allows `.obsidian/**/*.json` (other plugins' `data.json` are tracked), so the new config will
  travel with the vault once committed by the normal bob git flow.
- The earlier `sase_plan_metadata_menu_property_suggestions.md` recommended scoped fileClasses for enum-like fields
  (`status`, etc.) but explicitly deferred `type`. This plan uses a single global _preset field_ for `type` only, which
  doesn't conflict with (or require) the fileClass work.

## Design Decisions (flagged for review)

1. **Monthly and yearly notes get `type: "[[day]]"`** (not `[[month]]`/`[[year]]`), exactly as requested. `day.md`'s
   description will be rewritten to say it is the type for all periodic notes (daily, monthly, yearly).
2. **`done.md` parent changes `[[org]]` → `[[type]]`** and **`inbox.md` gains `parent: "[[type]]"`** so that "children
   of `[[type]]`" is the complete, authoritative list of types (this also powers the dropdown).
3. **A new `project.md` is created** rather than reusing `[[prj]]`/`[[projects]]`, keeping the existing `type: project`
   name; it will reference those notes as related.
4. **Dropdown = dynamic query for children of `[[type]]`**, not a hardcoded value list, so future types appear in the
   dropdown automatically once their note has `parent: "[[type]]"`.

## Implementation Plan

1. **Create/update type notes** (frontmatter style matching `type.md`/`day.md`: `parent: "[[type]]"` + `created`):
   - Create `restaurant.md`: describes restaurant notes (one per restaurant under `eat/`, tracking liked/not-liked
     verdicts, cuisine, location); links to `[[eat]]` and `[[maybe_eat]]` as related notes.
   - Create `project.md`: describes project notes (a note per active project, e.g. `[[job]]`, `[[obsidian]]`); links to
     `[[prj]]` and `[[projects]]` as related notes.
   - Update `day.md` body: type note for all periodic notes — daily (`YYYYMMDD`), monthly (`YYYYMM`), and yearly
     (`YYYY`).
   - Update `done.md`: `parent: "[[org]]"` → `parent: "[[type]]"`.
   - Update `inbox.md`: add `parent: "[[type]]"`.

2. **Migrate existing plain-string `type` values** with a small throwaway Python script (dry-run first; only rewrites
   the matched `type:` line):
   - `daily`/`monthly`/`yearly` → `"[[day]]"`; `restaurant` → `"[[restaurant]]"`; `project` → `"[[project]]"`; `inbox` →
     `"[[inbox]]"`.

3. **Backfill periodic notes**: same script inserts `type: "[[day]]"` (as the last frontmatter key) into every
   `YYYYMMDD.md`/`YYYYMM.md`/`YYYY.md` file that lacks a `type` key, anywhere in the vault outside `.obsidian/`,
   `_generated/`, and `done/`. Idempotent: files that already have `type` are only touched by step 2's rewrite.

4. **Update templates**: `_templates/monthly.md` and `_templates/yearly.md` get `type: "[[day]]"` (`_templates/daily.md`
   already has it).

5. **Update `eat.base`**: change the filter `note.type == "restaurant"` to link-aware form
   (`note.type == link("restaurant")`, per Obsidian Bases' `link()` function; verify exact syntax against the Bases
   docs/`ref/docs/obsidian_bases.md` during implementation).

6. **Configure Metadata Menu**: add one global preset field to `.obsidian/plugins/metadata-menu/data.json` →
   `presetFields`:
   - name `type`, input type `File` (single-link dropdown), with a `dvQueryString` selecting children of `[[type]]`,
     e.g.
     `dv.pages().where(p => { const par = p.parent; const links = Array.isArray(par) ? par : (par ? [par] : []); return links.some(l => l && l.path === "type.md"); })`
   - Verify the exact field JSON shape (id format, option keys) against the installed plugin (v0.8.12) before writing,
     since the file is edited directly rather than through the settings UI.
   - Do this step last and note that Obsidian must be reloaded to pick it up (and that an open Obsidian instance could
     overwrite a concurrent edit).

7. **Verify**:
   - `bob dataview` grouped query: the only distinct `type` values are the six links (`[[day]]`, `[[done]]`, `[[ref]]`,
     `[[restaurant]]`, `[[project]]`, `[[inbox]]`).
   - `grep -rE '^type: [^"]' --include='*.md'` over the vault (excluding `.obsidian/`) returns nothing.
   - Counts reconcile: ~1,066 notes with `type: "[[day]]"`; 91 with `[[restaurant]]`; totals match the inventory above.
   - `bob dataview` equivalent of the eat.base filter (`FROM "eat" WHERE type = [[restaurant]]`) still returns 91
     restaurants.
   - Bryan confirms in Obsidian that eat.base still renders all restaurants and that the `type` dropdown lists the six
     type notes.

8. **Commits**: vault changes are left for the normal bob git flow (`bob bulk-git-commit` / nightly) — the vault already
   has unrelated uncommitted edits, so this plan does not run its own vault commit. No bob-cli source changes are
   needed.

## Out of Scope

- Weekly notes (`2023_week_*.md`), habit notes (`*_habit*.md`), old-format month notes (`2023_july.md` etc.), legacy
  done archives (`*_done.md` without `type`), and the stray empty `2026-05-28.md`.
- Assigning `type` to non-periodic notes that don't have one today.
- The broader fileClass taxonomy for `status`/`ref_type`/`category` from the earlier Metadata Menu plan.
