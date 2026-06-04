---
status: pending
create_time: 2026-06-04 15:13:13
prompt: sdd/prompts/202606/highlights_ref_task_priority.md
---

# Plan: Add `[p::2]` to `bob highlights` Reference Tasks

## Context

`bob highlights` generates Obsidian reference notes under `~/bob/ref` from Highlights PDF marker notes. The generated
PDF affordance is currently an Obsidian task line like:

```md
- [ ] #task [[lib/books/example.pdf]] ^task
```

The requested behavior is to start appending the priority inline field `[p::2]` to these generated reference-task lines
and to update the existing generated-looking tasks already present under `~/bob/ref`.

This is not a CLI surface change. No new `bob highlights` subcommands, flags, or option semantics are being added, so
the CLI-rules long-term memory is not required for this work.

## Current Findings

- Obsidian vault context was read through `sase memory read long/obsidian.md`.
- The relevant implementation is centralized in `src/native/highlights_ref/mod.rs`.
- New-note body rendering happens in `default_note_body()`.
- Existing-note body preservation flows through `ParsedNote::render_body()`, `parse_pdf_task_line()`, and
  `rewrite_pdf_task_checkbox()`.
- The parser is token-based: it finds the generated PDF task line by `^task`, then requires a Markdown checkbox,
  `#task`, and a PDF wikilink. That means adding `[p::2]` should not break existing parsing as long as `^task` remains a
  separate token.
- Existing `~/bob/ref` generated-looking PDF task inventory currently has 11 lines, none with `[p::2]`:
  - `ref/blogs/steve_kinney_agent_memory.md`
  - `ref/chat/bulk_obsidian_task_properties.md`
  - `ref/chat/highlights-ref-sync.md`
  - `ref/chat/obsidian-note-refactor-plugin-consolidated.md`
  - `ref/chat/sase_blog_launch_strategy_consolidated.md`
  - `ref/chat/zorg-reference-notes-obsidian-native-options.md`
  - `ref/docs/obsidian_bases.md`
  - `ref/docs/obsidian_docs.md`
  - `ref/papers/human_mem_arch.md`
  - `ref/papers/log_is_the_agent.md`
  - `ref/papers/memory_os.md`
- Those 11 files are already dirty or untracked in the `~/bob` Git worktree, so migration edits must be line-scoped and
  must not reset, stash, clean, or otherwise overwrite unrelated user changes.

## Product Semantics

The canonical generated line should become:

```md
- [ ] #task [[lib/books/example.pdf]] [p::2] ^task
```

For read notes, preserve the current checked form:

```md
- [x] #task [[lib/books/example.pdf]] [p::2] ^task
```

For existing tasks that already contain completion/cancelled metadata, insert `[p::2]` after the PDF wikilink and before
the status date fields / final `^task`, for example:

```md
- [x] #task [[lib/chat/example.pdf]] [p::2] [completion:: 2026-06-04] ^task
```

This keeps the stable Obsidian block ID as the final token and matches the vault's broader convention of keeping
priority near the task text before completion/cancelled metadata. The migration should be idempotent and must not add a
second `[p::2]` if one appears before edits are applied.

## Implementation Steps

1. Update command rendering.
   - Add a small constant for the generated task priority, e.g. `PDF_TASK_PRIORITY: &str = "[p::2]"`.
   - Update `default_note_body()` in `src/native/highlights_ref/mod.rs` so new generated notes render
     `[[source_pdf]] [p::2] ^task`.
   - Keep the existing checked vs unchecked behavior based on `status: read`.

2. Preserve and verify parser/rewrite compatibility.
   - Keep `parse_pdf_task_line()` backward-compatible with task lines that do not yet have `[p::2]`; older notes should
     not fail just because they have not been migrated.
   - Update the malformed-line diagnostic to show the new canonical task form.
   - Update unit tests so parser coverage includes the new priority field.
   - Update checkbox rewrite tests so toggling `[ ]` to `[x]` preserves `[p::2]`.

3. Update integration tests and docs.
   - Update generated-note assertions in `tests/cli.rs` to expect `[p::2]`.
   - Update tests that sync marker/frontmatter/read-status changes and assert the checked generated task line.
   - Update `docs/highlights-ref-sync.md` generated-note examples and related prose.
   - Update `README.md` only if it contains an active generated-task example that needs the new field.

4. Migrate existing vault tasks under `~/bob/ref`.
   - Re-run the targeted preflight immediately before editing:
     - `git -C ~/bob status --short -- ref`
     - `rg -n '^- \\[[^\\]]*\\] #task \\[\\[.*\\.pdf.*\\]\\].*\\^task' ~/bob/ref -g '*.md'`
     - `rg -n '\\[p::2\\]' ~/bob/ref -g '*.md'`
   - Edit only the 11 generated-looking PDF task lines currently found under `~/bob/ref`.
   - Insert `[p::2]` after the PDF wikilink, preserving checkbox state, completion/cancelled metadata, spacing where
     practical, and the final `^task` block ID.
   - Do not edit unrelated `~/bob/ref` content, non-PDF tasks, frontmatter, highlight regions, or backlinks.

5. Verification.
   - Run `cargo fmt --check`.
   - Run focused Rust tests:
     - `cargo test highlights_ref_task_line_parser`
     - `cargo test highlights_ref_task_checkbox_rewrite`
     - `cargo test --test cli highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note`
     - `cargo test --test cli highlights_ref_marker_edit_updates_frontmatter`
     - `cargo test --test cli highlights_ref_deprecated_done_status_migrates_to_read_with_pdf_write`
     - `cargo test --test cli highlights_ref_task_checked`
     - `cargo test --test cli highlights_ref_sync_renders_sidecar_highlights_and_notes`
   - If focused failures suggest broader fallout, run `cargo test highlights_ref` and/or
     `cargo test --test cli highlights_ref`.
   - Verify vault migration with:
     - `rg -n '^- \\[[^\\]]*\\] #task \\[\\[.*\\.pdf.*\\]\\](?!.*\\[p::2\\]).*\\^task' ~/bob/ref -g '*.md'` if ripgrep
       look-around is available, or an equivalent non-lookaround check.
     - `rg -n '^- \\[[^\\]]*\\] #task \\[\\[.*\\.pdf.*\\]\\].*\\[p::2\\].*\\^task' ~/bob/ref -g '*.md'`
   - Review diffs for both the repo and `~/bob` to confirm edits are limited to the planned task-priority changes.

## Risks and Mitigations

- Risk: placing `[p::2]` after `^task` could destabilize Obsidian block links. Mitigation: keep `^task` as the final
  token and insert priority before it.
- Risk: existing completed/cancelled task metadata could be reordered or removed. Mitigation: insert priority after the
  PDF wikilink and preserve all remaining text.
- Risk: dirty `~/bob` files contain unrelated user edits. Mitigation: only edit the single generated task line in each
  target file and never run destructive Git commands.
- Risk: making the parser require `[p::2]` would reject older generated notes in other vaults. Mitigation: render the
  new canonical form while continuing to parse older lines.

## Acceptance Criteria

- New `bob highlights` reference notes include `[p::2]` on the generated PDF task line.
- Existing generated-looking PDF task lines under `~/bob/ref` include `[p::2]`.
- Existing checked/cancelled/completion metadata and `^task` block IDs are preserved.
- Parser and checkbox rewrite behavior continue to work with the priority field.
- Focused highlights tests pass or any remaining test failure is clearly unrelated and documented.
