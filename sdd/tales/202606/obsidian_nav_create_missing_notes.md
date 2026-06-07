---
create_time: 2026-06-07 09:36:40
status: wip
prompt: sdd/prompts/202606/obsidian_nav_create_missing_notes.md
---
# Plan: Missing Note Creation for Parent and Prev/Next Navigation

## Goal

Extend Bob's Obsidian navigation behavior so:

- `Ctrl+-` / `bob-navigation-hotkeys:open-parent-note` opens an existing `parent` target or creates it when it is a safe
  missing Markdown note target.
- `[[` / `bob-navigation-hotkeys:open-prev-link` and `]]` / `bob-navigation-hotkeys:open-next-link` do the same for the
  first rendered `prev` / `next` body links.
- Creation uses the same guarded Templater path and template selection already used by the Vim Enter link action.

The motivating case is a daily note whose frontmatter has `parent: [[YYYY/YYYYMM]]`; on the first day of a month,
`Ctrl+-` should create `~/bob/YYYY/YYYYMM.md` using `_templates/monthly.md` when that monthly note does not already
exist.

## Context Reviewed

- Required Obsidian memory was read through `sase memory read long/obsidian.md`.
- The relevant implementation is the live vault plugin:
  - `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- The Enter creation path already exists in `bob-navigation-hotkeys`:
  - line-link parsing builds open/create candidates;
  - missing safe Markdown note targets call `createNoteFromLinkCandidate`;
  - creation uses Templater's `create_new_note_from_template`;
  - template selection is path-based:
    - `YYYY/YYYYMMDD.md` and legacy `_day` daily paths use `_templates/daily.md`;
    - `YYYY/YYYYMM.md` uses `_templates/monthly.md`;
    - `YYYY.md` uses `_templates/yearly.md`;
    - everything else safe uses `_templates/new_note.md`.
- Current `openParentNote()` and `openLabeledBodyLink("prev" | "next")` still call `openResolvedLink()`, which only
  opens an existing resolved target and shows a not-found notice when resolution fails.
- The tracked `.obsidian.vimrc` maps `[[` and `]]` to the prev/next Obsidian commands, but the live vault currently has
  `.obsidian.vimrc` deleted. I will not restore or alter that user state as part of this change; the command behavior is
  the target.
- The vault is already dirty. Existing unrelated changes must be preserved.

## Design

Keep all new behavior inside `bob-navigation-hotkeys/main.js`.

Add a shared command-level helper that takes a link target plus source path, converts it into the same open/create
candidate shape used by Enter, and then calls `openOrCreateLinkCandidate()`:

- If the target resolves to an existing Markdown note, open it through the existing `openResolvedLink()` flow so
  subpaths keep working.
- If the target does not resolve but is a safe vault-relative Markdown note target, create it through
  `createNoteFromLinkCandidate()`.
- If the target is unsafe, external, non-Markdown, empty, or otherwise not creatable, preserve the current not-found
  notice behavior.

Use that helper in:

- `openFrontmatterLink(fieldName, missingMessage, notFoundMessage)` for `parent` and `template`.
  - This means `open-template-note` can also create a missing safe template link if one is present. If this broader
    behavior is undesirable during implementation review, split parent into its own helper instead.
- `openLabeledBodyLink(label)` for `prev` and `next`.

No duplicated template logic should be added. The existing `getNoteTemplateForCreationPath()` and
`createNoteFromLinkCandidate()` stay authoritative.

## Implementation Steps

1. Patch `/home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`.
2. Add a small helper, likely named `openOrCreateLinkTarget(linkTarget, sourcePath, notFoundMessage, renderedText)`.
3. Have the helper build a candidate with the existing `toLineLinkCandidate()` logic, passing a lightweight link object
   such as `{ target: linkTarget, renderedText }`.
4. If candidate construction fails, show the provided not-found notice and return `false`.
5. If a candidate exists, call `openOrCreateLinkCandidate(candidate)` and return its result.
6. Change `openFrontmatterLink()` to call the new helper after extracting the field link.
7. Change `openLabeledBodyLink()` to call the new helper after finding the rendered `prev` / `next` link.
8. Keep existing missing-field notices unchanged:
   - `No parent link found`
   - `No template link found`
   - `No prev link found`
   - `No next link found`
9. Avoid edits to `.obsidian/hotkeys.json`, `.obsidian.vimrc`, templates, manifests, Rust source, and unrelated dirty
   vault files unless implementation reveals a direct blocker.

## Validation

Automated or command-line checks:

- `node -c /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/main.js`
- `jq '.' /home/bryan/bob/.obsidian/plugins/bob-navigation-hotkeys/manifest.json`
- `git -C /home/bryan/bob diff -- .obsidian/plugins/bob-navigation-hotkeys/main.js`

Manual/live acceptance in Obsidian:

- From a daily note with `parent: [[YYYY/YYYYMM]]` where the monthly note is missing, press `Ctrl+-`; confirm the
  monthly note is created from `_templates/monthly.md`, opened, and has monthly frontmatter.
- Repeat `Ctrl+-` when the monthly note already exists; confirm it opens without rewriting or recreating the file.
- From a daily note whose `next` link points to a missing daily note, press `]]`; confirm the daily note is created from
  `_templates/daily.md`.
- From a daily note whose `prev` link points to a missing daily note, press `[[`; confirm the daily note is created from
  `_templates/daily.md`.
- Confirm existing `Enter` link open/create behavior still works.
- Confirm unsafe or non-note targets still do not create files and show a notice.

## Risks and Mitigations

- Templater's internal creation API is not a formal public API. This is already a dependency of the Enter flow, so the
  change reuses the guarded existing path rather than introducing a new dependency.
- Creating through `open-template-note` may be broader than requested if `openFrontmatterLink()` is changed generically.
  If that feels too broad after patch review, keep `template` on open-only behavior and apply create support only to
  `parent`, `prev`, and `next`.
- The live `.obsidian.vimrc` deletion may prevent `[[` / `]]` from being active in the current vault session. This plan
  changes the commands invoked by those mappings and does not restore the deleted vimrc file.
- The vault has substantial unrelated dirty state. Limit the diff to `bob-navigation-hotkeys/main.js` and inspect the
  final diff carefully.
