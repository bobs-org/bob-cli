---
create_time: 2026-06-04 16:08:22
status: wip
prompt: sdd/prompts/202606/highlights_ref_cancelled_task.md
---
# Plan: Accept Cancelled Generated Highlights Tasks

## Diagnosis

`bob highlights scan` fails while planning `lib/chat/bulk_obsidian_task_properties.pdf` because the existing reference
note contains a generated PDF task line whose checkbox marker is `[-]`:

```md
- [-] #task [[lib/chat/bulk_obsidian_task_properties.pdf]] [p::2] [cancelled:: 2026-06-04] [p::2] ^task
```

The current generated-task parser in `src/native/highlights_ref/mod.rs` only accepts `[ ]`, `[x]`, and `[X]`. When it
sees `[-]`, parsing fails before sync planning can resolve frontmatter, marker state, or rewrite the generated task
checkbox. The recent priority change did not make `[p::2]` mandatory in parser logic; it only changed new rendering and
the malformed-line diagnostic. That updated diagnostic makes this failure look priority-related, but the root cause is
the cancelled checkbox marker.

There is also a duplicate `[p::2]` on the observed local note. The parser is token-based and does not reject duplicate
priority tokens, so that duplicate is not the immediate scan failure. It is still worth keeping tests around metadata
after the PDF link so future rewrites do not drop completion/cancelled fields.

This does not add CLI subcommands or options, so the `cli_rules` long-term memory is not required.

## Desired Behavior

Generated PDF task lines with Obsidian Tasks' cancelled marker `[-]` should be recognized as generated task lines, not
treated as malformed, when they also contain the stable `^task` block ID, `#task`, and a PDF wikilink.

Cancelled generated tasks should not contribute the checked-task `status: read` signal. They should behave like
unchecked tasks for sync planning. If the final resolved projection is `status: read`, the existing rewrite path may
normalize the marker to `[x]`; otherwise it may normalize the marker to `[ ]`. Existing inline metadata such as
`[cancelled:: YYYY-MM-DD]`, `[completion:: YYYY-MM-DD]`, and `[p::2]` should be preserved by marker-only rewrites
because the rewrite changes only the checkbox character.

The malformed-line diagnostic should be made less misleading by mentioning the accepted cancelled form, while still
keeping the parser narrower than a full Obsidian Tasks parser.

## Implementation Steps

1. Update the generated PDF task parser.
   - Extend `parse_markdown_task_checkbox()` so `'-'` is accepted as a valid checkbox marker with `checked == false`.
   - Keep `[x]` and `[X]` as the only markers that mean checked/read.
   - Continue rejecting arbitrary custom status markers such as `[/]`, `[>]`, or `[?]` unless there is a concrete
     generated-task workflow for them.

2. Improve the malformed-line error text.
   - Update `malformed_pdf_task_line_error()` to list `[ ]`, `[x]`, and `[-]` generated task forms with `[p::2]`.
   - Avoid implying that old no-priority generated lines are rejected, since the parser still accepts them for
     compatibility.

3. Add focused unit tests in `src/native/highlights_ref/mod.rs`.
   - Confirm `parse_pdf_task_line()` recognizes a cancelled generated task with `[cancelled:: YYYY-MM-DD]`, `[p::2]`, a
     PDF wikilink, and `^task`.
   - Confirm the cancelled marker is represented as present but not checked.
   - Confirm `rewrite_pdf_task_checkbox()` can rewrite cancelled generated task lines to checked and unchecked forms
     without dropping inline metadata.

4. Add CLI regression coverage in `tests/cli.rs`.
   - Create a temp vault PDF/ref note pair where the existing generated task line is `[-]` with cancelled metadata.
   - Run `bob highlights scan --dry-run` or targeted `sync --dry-run` and assert the command plans successfully rather
     than reporting a malformed task line.
   - Prefer a dry-run assertion for the regression so the test proves planning succeeds without relying on PDF write
     behavior.

5. Update documentation.
   - Amend `docs/highlights-ref-sync.md` to state that `[x]`/`[X]` is the only task state that contributes
     `status: read`.
   - Note that `[-]` cancelled generated task lines are tolerated as existing Obsidian Tasks metadata but do not infer a
     replacement status.

6. Verify.
   - Run `cargo fmt --check`, applying `cargo fmt` first if needed.
   - Run focused tests:
     - `cargo test highlights_ref_task_line_parser`
     - `cargo test highlights_ref_task_checkbox_rewrite`
     - the new CLI regression test by name
   - Run a broader highlights CLI filter if the regression touches shared sync planning behavior.
   - Run `git diff --check`.
