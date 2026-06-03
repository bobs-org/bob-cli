---
create_time: 2026-06-03 19:22:57
status: wip
prompt: sdd/prompts/202606/native_dataview_folder_source_parity.md
---
# Native Dataview Folder Source Parity

## Context

The native engine reproduces the reported query as a successful but empty result. The Obsidian engine returns rows for
the same query. The native JSON probe shows that `FROM "ref"` currently resolves to the exact note `ref.md`, so
`WHERE source_path AND url` eliminates the only selected row. In the live vault, `ref.md` and the `ref/` folder both
exist, and Dataview treats the quoted source as the folder for this query.

## Plan

1. Adjust native quoted path source resolution so a source like `"ref"` can select folder descendants when a matching
   folder exists, instead of returning the exact `ref.md` note first.
2. Preserve exact-note behavior where it is still needed by keeping a fallback for quoted sources that do not match any
   folder descendants.
3. Add regression coverage using a fixture that has both `ref.md` and `ref/`, proving `FROM "ref"` selects the folder
   rows and not the top-level note.
4. Add coverage for the reported shape:
   `LIST WITHOUT ID title + " (" + url + ")" FROM "ref" WHERE source_path AND url AND parent-chain... SORT title`.
5. Run focused Dataview tests, then run the exact live-vault native query with `--format markdown` to confirm it emits
   rows.
6. Run the repo quality target appropriate for the touched area, at minimum `just test` if the focused tests pass.
