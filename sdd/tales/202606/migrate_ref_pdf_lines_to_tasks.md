---
create_time: 2026-06-03 23:13:58
status: wip
prompt: sdd/prompts/202606/migrate_ref_pdf_lines_to_tasks.md
---
# Migrate non-legacy ref-note PDF lines to generated tasks

## Objective

Ensure every existing non-legacy `type: [[ref]]` note in `~/bob` uses the generated PDF task affordance:

```md
- [ ] #task [[path/to/file.pdf]] ^task
```

instead of the older plain `PDF: [[...pdf]]` line. Do this as a one-time Bob vault note migration, not as a `bob-cli`
code change.

## Current findings

I treated "non-legacy ref notes" as notes under `~/bob/ref` where Dataview reports `type = [[ref]]` and
`status != "legacy"`.

Dataview currently reports 292 ref notes:

- `legacy`: 282
- `wip`: 6
- `read`: 3
- `abandoned`: 1

The 10 non-legacy notes are:

- `ref/blogs/steve_kinney_agent_memory.md` (`wip`, `source_pdf: lib/blogs/steve_kinney_agent_memory.pdf`)
- `ref/chat/highlights-ref-sync.md` (`read`, `source_pdf: lib/chat/highlights-ref-sync.pdf`)
- `ref/chat/obsidian-note-refactor-plugin-consolidated.md` (`read`,
  `source_pdf: lib/chat/obsidian-note-refactor-plugin-consolidated.pdf`)
- `ref/chat/sase_blog_launch_strategy_consolidated.md` (`wip`,
  `source_pdf: lib/chat/sase_blog_launch_strategy_consolidated.pdf`)
- `ref/chat/zorg-reference-notes-obsidian-native-options.md` (`read`,
  `source_pdf: lib/chat/zorg-reference-notes-obsidian-native-options.pdf`)
- `ref/docs/obsidian_bases.md` (`wip`, `source_pdf: lib/docs/obsidian_bases.pdf`)
- `ref/docs/obsidian_docs.md` (`abandoned`, `source_pdf: lib/docs/obsidian_docs.pdf`)
- `ref/papers/human_mem_arch.md` (`wip`, `source_pdf: lib/papers/human_mem_arch.pdf`)
- `ref/papers/log_is_the_agent.md` (`wip`, `source_pdf: lib/papers/log_is_the_agent.pdf`)
- `ref/papers/memory_os.md` (`wip`, `source_pdf: lib/papers/memory_os.pdf`)

Current body state:

- `ref/docs/obsidian_bases.md` already has `- [ ] #task [[lib/docs/obsidian_bases.pdf]] ^task`.
- The other 9 notes still have `PDF: [[...pdf]]`, with `steve_kinney_agent_memory.md` and `log_is_the_agent.md` using
  the old `^pdf` block ID.

Current vault safety notes:

- `~/bob` is already dirty. I must not clean, reset, stash, or overwrite unrelated changes.
- Target-specific dirty status currently includes:
  - modified: `ref/papers/log_is_the_agent.md`
  - untracked: `ref/blogs/steve_kinney_agent_memory.md`
  - untracked: `ref/docs/obsidian_bases.md`
  - untracked: `ref/papers/human_mem_arch.md`
- `ref/docs/obsidian_bases.md` points at a missing `lib/docs/obsidian_bases.pdf`; that is an existing source-PDF issue
  and should not block the body-line migration.
- `ref/papers/log_is_the_agent.md` has existing sync-related edits, including a malformed-looking frontmatter fragment
  around `url` / `highlights_marker_fields`. Do not silently repair that as part of this migration unless validation
  shows it is required for this objective.
- Existing backlinks to old `#^pdf` block IDs exist in `2026/20260603_day.md`; one already points at
  `ref/docs/obsidian_bases#^pdf`, which is stale because that note now uses `^task`.

## Migration design

For each non-legacy ref note with exactly one old PDF line:

```md
PDF: [[<source_pdf>]] PDF: [[<source_pdf>]] ^pdf
```

replace only that line with:

```md
- [ ] #task [[<source_pdf>]] ^task
```

Use `[x]` only if the note status is already `done`. Current candidates are `wip`, `read`, or `abandoned`, so every
replacement should be unchecked.

Preserve everything else byte-for-byte:

- frontmatter, including current `status` values such as `read`
- title lines
- manual sections such as `## Summary` and `## My Notes`
- generated Highlights regions
- existing highlight block IDs

Also update backlinks in `~/bob` that point to the old PDF-line block IDs for these notes:

- replace `#^pdf` with `#^task` for `ref/papers/log_is_the_agent`
- replace `#^pdf` with `#^task` for `ref/blogs/steve_kinney_agent_memory`
- replace the already-stale `ref/docs/obsidian_bases#^pdf` backlink with `ref/docs/obsidian_bases#^task`

This preserves Obsidian embeds/links after removing old `^pdf` blocks. Do not update unrelated `#^pdf` text if any
appears outside these known note references.

## Execution plan

1. Re-run read-only preflight immediately before editing:
   - `git -C ~/bob status --short`
   - Dataview query listing non-legacy ref notes and `source_pdf`
   - `rg -n '^PDF:' ~/bob/ref --glob '*.md'`
   - `rg -n '\\^pdf|#\\^pdf' ~/bob --glob '*.md'`

2. Edit only the 9 ref notes with old `PDF:` lines:
   - `ref/blogs/steve_kinney_agent_memory.md`
   - `ref/chat/highlights-ref-sync.md`
   - `ref/chat/obsidian-note-refactor-plugin-consolidated.md`
   - `ref/chat/sase_blog_launch_strategy_consolidated.md`
   - `ref/chat/zorg-reference-notes-obsidian-native-options.md`
   - `ref/docs/obsidian_docs.md`
   - `ref/papers/human_mem_arch.md`
   - `ref/papers/log_is_the_agent.md`
   - `ref/papers/memory_os.md`

3. Edit only the known `#^pdf` backlinks in `2026/20260603_day.md` to `#^task`.

4. Run verification:
   - `rg -n '^PDF:' ~/bob/ref --glob '*.md'` should return no matches.
   - `rg -n '\\^pdf|#\\^pdf' ~/bob --glob '*.md'` should return no matches, or only explicitly unrelated matches if new
     ones appear during preflight.
   - `rg -n '^- \\[[ xX]\\] #task \\[\\[.*\\.pdf\\]\\] \\^task$' ~/bob/ref --glob '*.md'` should show the 10 non-legacy
     task lines.
   - Re-run the Dataview non-legacy ref-note query and confirm the same 10 notes remain non-legacy with unchanged
     statuses and `source_pdf` values.
   - Review `git -C ~/bob diff -- <edited files>` and confirm diffs are limited to the PDF-line-to-task replacements
     plus the planned backlink fragment changes.

5. Optional sync validation, without writing PDFs:
   - Use the current repo-built `bob highlights-ref` binary or build it after this plan is approved.
   - Run representative `sync --dry-run` checks for migrated notes.
   - Do not run `--write-pdf`.
   - Treat failures about existing marker/frontmatter state, missing `obsidian_bases.pdf`, or unrelated dirty files as
     diagnostics, not as permission to broaden the migration.

## Non-goals

- Do not modify legacy ref notes.
- Do not rewrite frontmatter statuses, including existing `read` statuses.
- Do not repair unrelated frontmatter or generated-highlight content.
- Do not write or rewrite PDFs.
- Do not run a writing `bob highlights-ref scan`.
- Do not commit, stage, stash, reset, or clean `~/bob` unless explicitly asked.

## Rollback

Because the vault is already dirty and includes untracked target notes, avoid destructive Git rollback commands. If
rollback is needed, apply the inverse textual replacements only to the files touched by this migration:

- task line back to its original `PDF:` line form for the 9 migrated notes
- `#^task` back to `#^pdf` in the backlink lines changed by this migration

For tracked clean files, `git -C ~/bob diff -- <file>` can identify the exact inverse patch. For preexisting dirty or
untracked files, use the preflight inventory and final diff instead of resetting.
