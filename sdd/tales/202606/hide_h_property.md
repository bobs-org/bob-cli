---
create_time: 2026-06-08 09:13:18
status: wip
prompt: sdd/prompts/202606/hide_h_property.md
---
# Plan: Hide the `h` Highlight Marker in Obsidian

## Objective

Make the generated highlight-task marker `[h:: ...]` invisible in Obsidian while keeping it in the Markdown file so
`bob highlights` can continue using it as the durable processed-task marker.

The desired user-facing result is:

- Highlight-created tasks still contain `[h:: <processed-id>]` on disk.
- Obsidian Reading mode and Live Preview do not show the `h` field on task lines.
- Any future frontmatter/YAML property named `h` is also hidden from Obsidian's Properties UI.
- The existing `🔖` source backlink and `[created:: ...]` property remain visible.
- No task sync behavior changes, no migration of existing notes, and no removal of the processed marker.

## Context Found

- The `h` marker is currently emitted by `bob highlights` as a Dataview inline field, for example:
  `[[#^h-...|🔖]] [h:: ...] [created::2026-06-08]`.
- `docs/highlights-ref-sync.md` and `README.md` now correctly describe `[h:: ...]` as the durable processed marker.
  Removing or hiding it at generation time would weaken duplicate detection unless we replace it with another durable
  marker.
- Bryan's live Obsidian vault is `/home/bryan/bob`.
- The vault already has an enabled CSS snippet: `/home/bryan/bob/.obsidian/snippets/dataview-properties.css`.
- `/home/bryan/bob/.obsidian/appearance.json` enables the `dataview-properties` snippet, so editing that file should
  apply automatically after Obsidian notices the saved snippet.
- Dataview 0.5.68 is installed. Its local `main.js` renders inline fields with:
  - a wrapper: `.dataview.inline-field`
  - bracket field key spans with `data-dv-key` and `data-dv-norm-key`
  - live-preview value spans with `data-dv-key` and `data-dv-norm-key`
  - parenthesis/standalone value spans with `data-dv-key` and `data-dv-norm-key`
- The linked Obsidian forum thread hides built-in metadata properties with selectors like
  `.metadata-property[data-property-key="always_hidden"] { display: none; }`. That is directly useful for frontmatter
  properties, but our generated task marker is a Dataview inline field, so we also need Dataview-specific selectors.
- The vault currently has unrelated dirty note changes. The planned snippet file is clean. Any implementation must avoid
  staging or committing unrelated vault changes.

## Recommended Design

Use the existing enabled vault snippet as the primary solution.

Append a small, targeted rule to `/home/bryan/bob/.obsidian/snippets/dataview-properties.css` after the existing
Dataview inline-field styling:

```css
/* Hide Bob's highlight-task processed marker while keeping it indexed. */
.metadata-property[data-property-key="h"],
.dataview.inline-field:has(> .dataview.inline-field-key[data-dv-norm-key="h"]),
.dataview.inline-field:has(> .dataview.inline-field-value[data-dv-norm-key="h"]),
.dataview.inline-field:has(> .dataview.inline-field-standalone-value[data-dv-norm-key="h"]) {
  display: none !important;
}
```

Design notes:

- Target `data-dv-norm-key="h"` instead of the displayed key text. This is more robust than matching rendered text and
  follows Dataview's own canonicalized key attribute.
- Include the `.metadata-property[data-property-key="h"]` selector from the forum pattern so a frontmatter property
  named `h` is hidden too.
- Keep the selectors global instead of limiting them to `.markdown-preview-view` or `.is-live-preview`; this should also
  cover embeds, popovers, and Dataview-rendered task/query output where Dataview emits the same inline-field DOM.
- Use `display: none !important` because the existing snippet and Dataview plugin both style `.dataview.inline-field`.
  The hide rule must win consistently.
- Do not edit `appearance.json` unless implementation discovers the snippet is no longer enabled.

## Source Mode Caveat

CSS snippets can hide rendered DOM, but they do not remove raw Markdown from Obsidian Source mode. Dataview's own live
preview extension also intentionally reveals an inline field when the cursor selection overlaps that field so users can
edit it.

For this request, the first implementation should treat "hidden in Obsidian" as hidden in normal rendered Obsidian
surfaces: Reading mode, Live Preview when not actively editing the field, embeds, popovers, and rendered task/query
output.

If visual verification shows that cursor-over-field or Source mode visibility is unacceptable, use a second-stage
implementation:

1. Add a small CodeMirror decoration extension to an existing Bob vault plugin rather than editing Dataview's vendored
   plugin file.
2. Match only strict `[h:: ...]` / `(h:: ...)` inline fields on Markdown task lines.
3. Replace the matched range with an empty widget or collapsed decoration in both Live Preview and Source mode.
4. Keep a helper-level test for range extraction and run `node -c` on the edited plugin.

That second stage is intentionally not the default because it changes editor behavior and makes the hidden marker harder
to inspect or repair manually.

## Implementation Steps

1. Confirm the starting state again.
   - `git -C /home/bryan/bob status --short --branch`
   - Confirm `.obsidian/snippets/dataview-properties.css` is still clean or only contains user changes we can preserve.
   - Confirm `.obsidian/appearance.json` still enables `dataview-properties`.

2. Patch the enabled snippet.
   - Append the hide rule to `/home/bryan/bob/.obsidian/snippets/dataview-properties.css`.
   - Keep the existing Dataview styling intact.
   - Do not edit vault notes or bob-cli Rust code.

3. Validate the CSS and file scope.
   - `git -C /home/bryan/bob diff --check -- .obsidian/snippets/dataview-properties.css .obsidian/appearance.json`
   - Inspect the diff and verify only the intended snippet rule changed.
   - Re-run `rg -n "\\[h::" /home/bryan/bob --glob "*.md"` to identify real notes for visual verification; `sase.md`
     already has examples.

4. Visual verification in Obsidian.
   - Open or reload Obsidian after saving the snippet.
   - Open `/home/bryan/bob/sase.md`, which currently has highlight-created tasks with `[h:: ...]`.
   - In Reading mode, verify the `h` chip is gone while the `🔖` source link and `[created:: ...]` remain visible.
   - In Live Preview, verify the same normal display.
   - If the snippet does not apply, reload CSS snippets or run Obsidian's reload command and recheck.

5. Commit only the vault snippet change after implementation.
   - Because this edits `/home/bryan/bob`, follow the vault `AGENTS.md`: commit with `sase_git_commit`.
   - Stage/commit only `.obsidian/snippets/dataview-properties.css` unless `appearance.json` also had to be changed.
   - Leave unrelated dirty notes untouched.

## Files Planned For Change

- `/home/bryan/bob/.obsidian/snippets/dataview-properties.css`

## Files Not Planned For Change

- No `bob-cli` Rust source.
- No generated reference notes or user task lines.
- No `README.md` or `docs/highlights-ref-sync.md` unless a later documentation note is explicitly requested.
- No Dataview plugin vendor files.
- No `appearance.json` unless the snippet is unexpectedly disabled.

## Acceptance Criteria

- `[h:: ...]` remains present in Markdown files and remains available to Dataview/bob-cli.
- Obsidian-rendered task lines do not show the `h` field.
- Obsidian Properties UI does not show a frontmatter property named `h` if one exists later.
- Existing visible task metadata such as `[created:: ...]`, `[due:: ...]`, `[completion:: ...]`, and source backlinks is
  unaffected.
- The final vault commit contains only the intended snippet/settings file(s).

## Risks

- If Dataview changes its inline-field DOM attributes in a future release, the Dataview-specific selectors may need to
  be updated.
- If Dataview pretty inline-field rendering is disabled, `[h:: ...]` will render as raw Markdown and CSS alone cannot
  hide it.
- CSS alone does not fully hide raw Source mode text or Dataview's cursor-over-field editing reveal. That requires the
  optional CodeMirror extension described above.
