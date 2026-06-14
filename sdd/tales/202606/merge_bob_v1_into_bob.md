---
create_time: 2026-06-14 09:01:20
status: proposed
prompt: sdd/prompts/202606/merge_bob_v1_into_bob.md
---

# Plan: Merge `bob_v1.md` Back Into Canonical `bob.md`

## Objective

Make `/home/bryan/bob/bob.md` the canonical project note for Bob again by merging the current contents of
`/home/bryan/bob/bob_v1.md` into it, converting `bob.md` from an area note to a proper project note, deleting
`bob_v1.md`, and updating all Obsidian link forms that would otherwise point at the deleted note.

No Bob vault content should be edited until this plan is approved.

## Current Observations

- `/home/bryan/bob/bob.md` is tracked and currently modified relative to git.
- `/home/bryan/bob/bob_v1.md` is currently untracked and contains the project content that should become canonical.
- Current `bob.md` is an area-style note:
  - `parent: "[[gtd]]"`
  - `type: "[[area]]"`
  - `status: wip`
  - `done_tasks: "[[done/bob_done]]"`
  - It also has the heading `# bob (Bugyi's [[obsidian]])` and a GitHub pointer for `bob-cli`.
- Current `bob_v1.md` is a project-style note:
  - `parent: "[[bob]]"`
  - `template: "[[new_project]]"`
  - `type: "[[project]]"`
  - `status: wip`
  - `created: 2026-06-14T08:06:36-04:00`
  - `task_count: 12`
  - `open_task_count: 4`
  - It contains the project completion task `^prj`, a `## Tasks` section, and a `## Future Work` section.
- `bob_v1.md` cannot be copied verbatim into `bob.md` because its `parent: "[[bob]]"` would become a self-parent.
- `gtd.md` is an area note, so keeping `parent: "[[gtd]]"` on the converted `bob.md` gives the project an area ancestor
  and avoids the self-parent.
- Current exact `bob_v1` references found in the vault:
  - 16 wikilink/block-link references in daily notes:
    - `2026/20260607.md`: 1
    - `2026/20260608.md`: 2
    - `2026/20260611.md`: 6
    - `2026/20260612.md`: 3
    - `2026/20260614.md`: 4
  - 1 ordinary wikilink inside `bob_v1.md` itself: `[[bob_v1]]` in the obsolete sub-bullet
    `Move tasks from [[bob]] to [[bob_v1]]!`
- No markdown-style links to `bob_v1` were found in the current scan.
- `2026/20260614.md` is untracked, so its changes will not show up in a normal `git diff`.
- Existing unrelated modified files in `/home/bryan/bob` must be preserved and left untouched unless a link scan proves
  they contain a `bob_v1` target.

## Target Shape for `bob.md`

Convert `bob.md` into a project note using a merge of the current `bob.md` metadata/context and `bob_v1.md` project
content:

```yaml
---
parent: "[[gtd]]"
template: "[[new_project]]"
type: "[[project]]"
status: wip
created: 2026-06-14T08:06:36-04:00
done_tasks: "[[done/bob_done]]"
task_count: 12
open_task_count: 4
---
```

Then use this body structure:

1. Keep the `bob_v1.md` top-level project completion task with `^prj`.
2. Preserve the current `bob.md` heading and GitHub pointer as project context:
   - `# bob (Bugyi's [[obsidian]])`
   - `See [bob-cli](https://github.com/bbugyi200/bob-cli) on GitHub for details on the \`bob\` CLI tool.`
3. Copy the `## Tasks` and `## Future Work` sections from `bob_v1.md`, preserving task status, task metadata, inline
   fields, indentation, and all block IDs.
4. Rewrite the obsolete sub-bullet `Move tasks from [[bob]] to [[bob_v1]]!` to non-stale wording that does not create a
   self-link or a dead `bob_v1` link, for example:
   - `Keep the canonical project task list in this note!`

## Link Rewrite Rules

Handle all Obsidian link types that can target `bob_v1`:

1. Wikilinks:
   - `[[bob_v1]]` -> `[[bob]]`
   - `[[bob_v1|alias]]` -> `[[bob|alias]]`
   - `[[bob_v1#Heading]]` -> `[[bob#Heading]]`
   - `[[bob_v1#^block-id]]` -> `[[bob#^block-id]]`
2. Wikilink embeds:
   - `![[bob_v1]]` -> `![[bob]]`
   - `![[bob_v1#Heading]]` -> `![[bob#Heading]]`
   - `![[bob_v1#^block-id]]` -> `![[bob#^block-id]]`
3. Extension-qualified wikilinks, if a fresh scan finds any:
   - `[[bob_v1.md]]` -> `[[bob.md]]` or preferably `[[bob]]` where local style allows.
   - Preserve aliases, heading anchors, block anchors, and embed prefixes.
4. Markdown links and markdown image embeds, if a fresh scan finds any:
   - `[text](bob_v1.md)` -> `[text](bob.md)`
   - `[text](bob_v1.md#anchor)` -> `[text](bob.md#anchor)`
   - `![alt](bob_v1.md)` -> `![alt](bob.md)`
5. Frontmatter property values are included because they may contain wikilinks as strings.
6. Do not rewrite ordinary `[[bob]]`, `[[bob.md|bob]]`, or markdown links to the GitHub `bob-cli` repository.
7. Preserve block IDs exactly; update references because the blocks move back into `bob.md`.

## Implementation Steps

1. Re-run fresh pre-edit scans:
   - `rg -n --hidden --glob '*.md' 'bob_v1' /home/bryan/bob`
   - scans for exact `[[bob_v1...]]`, `![[bob_v1...]]`, `[...](...bob_v1...)`, and `![...](...bob_v1...)` forms.
2. Re-read `/home/bryan/bob/bob.md` and `/home/bryan/bob/bob_v1.md` immediately before editing.
3. Replace `/home/bryan/bob/bob.md` with the merged project-note content described above.
4. Update every vault reference that targets `bob_v1` to target `bob`, preserving the specific link type:
   - The known current daily-note block links become `[[bob#^...]]` or `![[bob#^...]]`.
   - Any additional `bob_v1` link forms found by the fresh scan are handled by the same rule.
5. Delete `/home/bryan/bob/bob_v1.md`.
6. Leave unrelated dirty files untouched unless they contain a `bob_v1` target discovered by the pre-edit scan.

## Verification

1. Confirm the deleted note is gone:
   ```bash
   test ! -e /home/bryan/bob/bob_v1.md
   ```
2. Confirm no stale `bob_v1` references remain in Markdown files:
   ```bash
   rg -n --hidden --glob '*.md' 'bob_v1' /home/bryan/bob
   ```
   Expected: no output.
3. Confirm all migrated block IDs now exist in `bob.md`:
   ```bash
   rg -n '\^(prj|delete-file-keymap|blank-line-keymaps|task-keymap|close-and-create-pom-task|note-property-keymap|move-tab-keymaps|rename-keymaps|sub-bullet-prj-tasks|project-parents|p1-to-prj-notes|workspaces|tasks-in-pdfs)\b' /home/bryan/bob/bob.md
   ```
   Expected: 13 block definitions.
4. Confirm the known daily-note block references now target `bob`:
   ```bash
   rg -n --hidden --glob '*.md' '!?\[\[bob#\^(delete-file-keymap|blank-line-keymaps|task-keymap|close-and-create-pom-task|note-property-keymap|move-tab-keymaps|rename-keymaps|sub-bullet-prj-tasks|project-parents|p1-to-prj-notes|tasks-in-pdfs)\]\]' /home/bryan/bob/2026
   ```
   Expected: the same 16 references that currently target `bob_v1`.
5. Confirm `bob.md` is now a project note and not self-parented:
   ```bash
   sed -n '1,16p' /home/bryan/bob/bob.md
   ```
   Expected: `parent: "[[gtd]]"`, `template: "[[new_project]]"`, `type: "[[project]]"`, and no `parent: "[[bob]]"`.
6. Review targeted diffs:
   ```bash
   git -C /home/bryan/bob diff -- bob.md 2026/20260607.md 2026/20260608.md 2026/20260611.md 2026/20260612.md
   ```
   Also inspect `2026/20260614.md` directly because it is untracked.
7. Re-check vault status:
   ```bash
   git -C /home/bryan/bob status --short --untracked-files=all
   ```
   Report touched files separately from pre-existing unrelated dirty files.

## Risks and Mitigations

- **Self-parent risk:** avoided by keeping `parent: "[[gtd]]"` instead of copying `parent: "[[bob]]"` from `bob_v1.md`.
- **Dead link risk:** mitigated by requiring `rg 'bob_v1'` to return no Markdown references after deletion.
- **Over-rewrite risk:** mitigated by targeting exact `bob_v1` note links only; ordinary `[[bob]]` links remain valid.
- **Untracked-file risk:** `bob_v1.md` and `2026/20260614.md` are untracked, so verification cannot rely only on git
  diff.
- **Block-link risk:** mitigated by preserving all block IDs in the merged `bob.md` before rewriting references back to
  `bob#^...`.
