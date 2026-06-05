---
create_time: 2026-06-05 15:03:03
status: wip
prompt: sdd/prompts/202606/enter_link_all_link_types.md
---
# Enter Link Jump/Create: Full Obsidian Link-Type Support (incl. Block Links)

## Goal

Make the Vim normal-mode Enter link jump/create keymap in the Bob Obsidian vault handle **every** Obsidian link type
that points at a note location, with explicit, verified support for **block links**. Today the keymap handles most
cross-file links but silently ignores an entire class of links — **same-file subpath links**, which includes same-file
**block** references — so pressing Enter on them does nothing useful.

This plan:

1. Confirms which link types already work (and locks them in with tests).
2. Fixes the real gap: same-file subpath links (`[[#Heading]]`, `[[#^blockid]]`, `![[#^blockid]]`, `[label](#^blockid)`)
   should jump to that heading/block within the current note.
3. Improves the multi-link picker so heading/block links to the same note are distinguishable.

## Context Reviewed

- Read project short memory `memory/short/sase.md` and Obsidian long memory via `sase memory read long/obsidian.md` (new
  notes need a `parent` frontmatter link; `~/bob` is the vault).
- Read the prior approved plan and its committed implementation:
  - `sdd/tales/202606/enter_link_jump_create.md`
  - Commit `d715955 feat: add Enter link jump creation`.
- Inspected the current implementation:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (link parsing, candidate building, resolve,
    open/create, both picker modals).
  - `/home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js` (the `<CR>` owner that delegates to the navigation
    plugin via `handleVimEnterLinkAction`).
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css` (`bob-cnp-*` picker styles).
  - `/home/bryan/bob/_templates/new_note.md` (Templater `parent` expression).
- Confirmed there are no committed Obsidian tests; verification is via ad-hoc Node VM checks with stubbed `obsidian` and
  `@codemirror/view` modules, matching the prior plan.
- Vault `AGENTS.md` rules still apply: inspect status first, preserve unrelated dirty files, and commit only
  task-related vault files with the SASE git commit workflow. No `bob-cli` Rust CLI changes, so
  `memory/long/cli_rules.md` is not required.

## Link-Type Audit (traced through the current code)

Legend: ✅ already works · ❌ broken/ignored (this plan fixes) · ⏭️ intentionally ignored.

### Cross-file wikilinks

- `[[note]]`, `[[folder/note]]`, `[[note|Alias]]` — ✅ open/create.
- `[[note#Heading]]`, `[[note#Heading#Sub]]` — ✅ open (subpath passed to `openLinkText`); create makes the base note.
- `[[note#^blockid]]` — ✅ **block link**: resolves the base note via `getFirstLinkpathDest` (subpath stripped at the
  first `#`), opens via `openLinkText("note#^blockid", …)` which scrolls to the block; if the note is missing it creates
  the base note `note.md`.
- `![[note]]`, `![[note#^blockid]]` — ✅ transclusion accepted (`allowTransclusion`), same resolution as above.

### Cross-file Markdown links

- `[L](note.md)`, `[L](folder/note.md)`, `[L](<note with spaces.md>)`, `[L](note%20name.md)` — ✅ (URI-decoded).
- `[L](note.md#Heading)`, `[L](note.md#^blockid)` — ✅ **block link** via the same strip-base-then-`openLinkText` path.
- `[L](note.md "title")` — ✅ (title stripped).

### Same-file subpath links — ❌ THE GAP (this is where block links break)

- `[[#Heading]]`, `[[#Heading#Sub]]` — ❌ no candidate produced.
- `[[#^blockid]]` — ❌ **same-file block link**: no candidate produced.
- `![[#^blockid]]` — ❌ same-file block embed: no candidate produced.
- `[L](#Heading)`, `[L](#^blockid)` — ❌ Markdown same-file heading/block: no candidate produced.

Root cause (traced): for a pure-subpath link the parser yields `target = "#^blockid"`. `resolveLinkTargetFile` computes
`lookupText = stripLinkSubpath("#^blockid") = ""` and returns `null` (empty lookup). `getCreationTargetForLinkTarget`
then sees an empty path part and rejects it as unsafe. So `toLineLinkCandidate` returns `null`, no candidate is
collected, and `handleVimEnterLinkAction` returns `false` → Enter falls through to task toggle / line movement. The link
never resolves to "the current file at this heading/block."

### Intentionally ignored (unchanged)

- External/URI links: `[L](https://…)`, `[[https://…]]` — ⏭️ `isExternalLinkTarget`.
- Non-note embeds: `![[image.png]]`, `[[file.pdf]]` — ⏭️ non-markdown targets.
- Bare `#tag`, footnotes `[^1]`, reference-style `[L][ref]` — ⏭️ not note-location links (out of scope; noted for
  completeness).

## Product Decisions

1. **Treat a same-file subpath link as "open the current note at that heading/block."** A link whose path part is empty
   but which carries a `#heading` or `#^blockid` subpath (wiki, transclusion, or Markdown form) resolves to the
   active/source file. Pressing Enter jumps there via Obsidian's native `openLinkText("#…", sourcePath)` (the same
   mechanism Obsidian uses when you click such a link).

2. **Same-file links never create.** A `#heading`/`#^blockid` cannot create a file or a block. These are always `open`
   candidates; if the heading/block does not exist, Obsidian simply opens/keeps the current note (no error, no file
   creation).

3. **Keep cross-file block-link behavior exactly as-is, and lock it in with tests.** `[[note#^blockid]]` and friends
   already open the resolved note and scroll to the block, and create the base note when missing. No behavior change;
   add regression coverage so it cannot silently break.

4. **Disambiguate heading/block candidates in the multi-link picker.** When a line has several heading/block links
   (especially several pointing at the same note, or several same-file blocks), the rows must be distinguishable.
   Surface the subpath (`#Heading` / `#^blockid`) in each row and include it in the filter text. Single-candidate
   behavior (immediate open, no picker) is unchanged.

5. **Centralize the fix in resolution, scoped by a guard.** Make `resolveLinkTargetFile` resolve a genuine pure-subpath
   link to the source file. Guard it so only a non-empty heading/block subpath qualifies (`[[#]]` / `[[#^]]` degenerate
   cases do not). This single change makes both candidate building and `openResolvedLink` work consistently, and is safe
   for the other callers (see Risks).

6. **No scope creep.** Do not touch `.obsidian/hotkeys.json`, `.obsidian.vimrc`, the Templater/vimrc plugins, or
   `bob-cli` Rust. The `_templates/new_note.md` template is already correct and needs no change for this task.

## Implementation Scope

Expected vault files to edit:

- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js` (primary)
- `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/styles.css` (only if the picker subpath cue needs a small
  style; reuse existing `bob-cnp-*` classes where possible)

No changes expected to `task-status-cycler/main.js` (it already delegates correctly) or `_templates/new_note.md`.

### `bob-navigation-hotkeys/main.js` changes

- **Subpath helpers (pure, exported for tests):**
  - `getLinkSubpath(linkText)` → returns the `#…` portion (heading and/or `^blockid`) or `""`.
  - `isSubpathOnlyLink(linkText)` → true when the path part is empty and a non-empty heading/block subpath is present
    (rejects `#`, `#^`, and the empty string).
  - Reuse the existing `findSubpathIndex` / `stripLinkSubpath` / `stripMarkdownExtension` helpers rather than adding new
    regex parsing.

- **`resolveLinkTargetFile(linkTarget, sourcePath)`:** when `lookupText` is empty, if `isSubpathOnlyLink(linkText)` and
  a `sourcePath` is given, return the active/source file (`this.app.vault.getAbstractFileByPath(sourcePath)` when it is
  a markdown file); otherwise keep returning `null`. Cross-file resolution is unchanged.

- **`toLineLinkCandidate(link, sourcePath, index)`:** with the resolution change, same-file links naturally take the
  existing `open` branch (resolvedFile = source file). Add a `subpath` field to every candidate (`getLinkSubpath` of the
  normalized target, possibly empty) so the picker can show it. Ensure same-file links never reach the creation branch
  (they always resolve).

- **Candidate label/identity:**
  - Carry `subpath` on candidates; keep the existing alias→basename label rule for the title.
  - The dedupe key already includes the full subpath for `open` candidates, so distinct headings/blocks to the same note
    remain separate rows — keep that, just confirm same-file links produce stable keys (`open:<sourcePath>:#…`).

- **`LinkCandidatePickerModal`:** render the `subpath` next to the path (e.g. `Notes/foo.md#^abc`, or `foo.md#Heading`
  for same-file), and include `subpath` in `filterItem`. Keep all existing keyboard/click/empty-state behavior.

- **Export** any new pure helpers under `module.exports.helpers` for Node VM tests.

### `bob-navigation-hotkeys/styles.css` (only if needed)

- Optional small style for the subpath cue in a picker row, reusing existing `bob-cnp-row-path` styling. Prefer no new
  class if the subpath can be appended to the existing path element.

## Acceptance Criteria

- Pressing Enter on a line whose only link is `[[#Heading]]` jumps to that heading in the current note.
- Pressing Enter on a line whose only link is `[[#^blockid]]` jumps to that block in the current note (the headline
  block-link fix).
- `![[#^blockid]]` and `[label](#^blockid)` / `[label](#Heading)` behave the same as their wiki equivalents.
- `[[note#^blockid]]`, `![[note#^blockid]]`, and `[label](note.md#^blockid)` still open the resolved note and scroll to
  the block (regression locked by tests).
- A missing cross-file block link `[[missing#^blockid]]` still creates the base note `missing.md` from
  `_templates/new_note.md` and opens it (block dropped on create).
- A line with multiple heading/block links opens the picker, and rows are distinguishable by their subpath; filtering on
  a heading/block string narrows the list; Enter/click opens the selected target.
- Same-file links never create a file and never show a "Create" cue.
- All previously working link types, the child-note picker, task-status Enter toggling, and the existing link commands
  (`open-parent-note`, `open-template-note`, `open-next-link`, `open-prev-link`, `toggle-line-transclusions`,
  `open-alternate-file`) are unchanged.

## Verification Plan

Static checks:

```bash
node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js
node -c /home/bryan/bob/.obsidian/plugins/task-status-cycler/main.js
jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json
git -C /home/bryan/bob diff --check -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/styles.css
```

Focused Node VM checks (stubbed `obsidian` + `@codemirror/view`, fake `app.vault.getAbstractFileByPath` /
`app.metadataCache.getFirstLinkpathDest`). Use JSON comparisons for cross-VM arrays (prior harness lesson):

- `isSubpathOnlyLink` / `getLinkSubpath`: true + correct subpath for `#Heading`, `#Heading#Sub`, `#^blockid`; false for
  `note`, `note#^blockid`, `#`, `#^`, empty.
- Same-file candidate building → single `open` candidate resolving to the source file, `subpath` set, never `create`,
  for each of: `[[#Heading]]`, `[[#^blockid]]`, `![[#^blockid]]`, `[L](#Heading)`, `[L](#^blockid)`.
- Cross-file block links → `open` candidate resolving to the target note, `subpath` set, for `[[note#^blockid]]`,
  `![[note#^blockid]]`, `[L](note.md#^blockid)`.
- Missing cross-file block link `[[missing#^blockid]]` → `create` candidate with path `missing.md`.
- Multiple heading/block links on one line → multiple candidates, distinct dedupe keys, picker (not immediate open);
  candidates carry distinct `subpath` values.
- Regression: `extractLinkTarget`/`frontmatterFieldPointsToFile` for a plain `parent: "[[note]]"` is unchanged, and a
  pure-subpath frontmatter value does not match an unrelated parent (no `collectChildNotes` false positives).
- Single resolved candidate opens immediately; single missing safe candidate hits the Templater create path (existing
  behavior preserved).

Manual live-vault acceptance check after implementation and plugin reload:

1. Scratch note with a defined block (`… ^abc`) and a heading; add lines with `[[#^abc]]`, `[[#Heading]]`, `![[#^abc]]`,
   `[x](#Heading)`.
2. Press Enter on each same-file link line → cursor/scroll jumps to that block/heading in the same note.
3. Press Enter on a `[[OtherNote#^abc]]` line → other note opens scrolled to the block.
4. Press Enter on a `[[Missing#^abc]]` line → new note created from the template with a `parent` link, opened.
5. Line with several block/heading links → picker appears, rows show distinct subpaths, filter + Enter open the chosen
   target.
6. Confirm task-status Enter toggle, child-note picker, and the existing link commands still behave as before.

Before finishing:

```bash
git -C /home/bryan/bob status --short -- \
  .obsidian/plugins/bob-navigation-hotkeys/main.js \
  .obsidian/plugins/bob-navigation-hotkeys/styles.css \
  .obsidian/hotkeys.json .obsidian.vimrc
git status --short
```

If vault files changed, commit only the task-related vault files with `/sase_git_commit`, leaving the pre-existing dirty
notes and `.obsidian/hotkeys.json` untouched.

## Risks

- **Changing shared `resolveLinkTargetFile`.** Other callers are `openResolvedLink`, `frontmatterFieldPointsToFile`
  (child collection), and `extractLinkTarget`-fed flows. A pure-subpath frontmatter value would now resolve to the
  source file itself, which can never equal a _different_ parent file, so `collectChildNotes` gains no false positives;
  body "open next/prev link" with a same-file aliased subpath now jumps in-file instead of erroring, which is an
  improvement. The guard (`isSubpathOnlyLink` requires a non-empty heading/block) prevents degenerate `#`/`#^` matches.
  Tests cover these.
- **Block existence is not validated.** As today for headings, we resolve the file and let `openLinkText` scroll; a
  missing block just opens the note without scrolling. Acceptable and consistent with current heading behavior.
- **Templater internal API** is unchanged by this plan; the create path is only reached by cross-file links exactly as
  before.
- **Picker row layout.** Adding a subpath cue must not regress the child-note picker (separate item shape) — only the
  link-candidate renderer changes.
