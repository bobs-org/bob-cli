---
create_time: 2026-06-16 10:22:37
status: wip
prompt: sdd/prompts/202606/capture_tasks_section.md
---
# Plan: Prefer `Tasks` Sections for `bob capture`

## Context

`bob capture` is implemented in `src/native/capture.rs`. Existing routed captures create `<route>.md` if missing;
otherwise they call `insert_task_line()`, which inserts after the last top-level Bob task block anywhere in the file. If
no such block exists, it appends at EOF. Unrouted inbox captures currently append to `mac_inbox.md`.

This behavior is wrong for project/area notes that have a root project task before `## Tasks`: the capture can land near
the root task instead of in the note's task section. The new rule should be: when the target file already has a Markdown
heading whose title is exactly `Tasks`, `bob capture` should prefer that section over the old global task-block rule.

No new CLI subcommands or options are needed, so the long-term CLI rules memory does not need to be read for this
change.

## Behavior

1. Apply the new placement rule to every existing capture target, including routed files and `mac_inbox.md`.
2. Preserve current behavior for brand-new files: write a single task line plus trailing newline.
3. For existing files, choose insertion in this order:
   1. If a `Tasks` section exists, insert into that section.
   2. Otherwise, keep the current fallback: insert after the last top-level Bob task block anywhere in the file.
   3. Otherwise, append at EOF with the existing newline guard.
4. Define a `Tasks` section as the first ATX Markdown heading outside YAML frontmatter and fenced code blocks whose
   normalized title is exactly `Tasks`.
   - Accept heading levels 1 through 6, so `# Tasks`, `## Tasks`, and `### Tasks` all count.
   - Strip optional closing heading markers, so `## Tasks ##` counts.
   - Treat the section's direct insertion area as ending before the next heading of any level. This keeps captures
     directly under `Tasks` and out of nested subsections like `### Future Work`.
5. Inside the `Tasks` section:
   - If top-level Bob task blocks already exist, insert after the last such task block and its indented continuation
     lines, reusing the current task-block semantics.
   - If no top-level Bob task block exists in the section, insert immediately after the section heading with exactly one
     blank line between the heading and the new task.
6. Keep the task line format, routing parser, JSON schema, output strings, and `--dry-run` behavior unchanged.
   - `placement` should be `inserted` when a `Tasks` section determines placement, even if the section is empty.
   - `placement` should remain `appended` only for the EOF fallback.

## Implementation Outline

1. Refactor `insert_task_line(contents, task_line)` in `src/native/capture.rs` into a small precedence pipeline:
   - `tasks_section_insert_index(contents)`
   - existing `last_task_block_insert_index(contents)`
   - EOF append fallback
2. Extend the existing `LineSpan` parsing rather than introducing a Markdown parser. The required Markdown subset is
   small and byte-span based insertion is already the local pattern.
3. Add a `Heading`/`Section` helper that scans `LineSpan`s while tracking:
   - YAML frontmatter only when it starts at the beginning of the file with `---`
   - fenced code blocks starting with at least three backticks or tildes
   - ATX headings outside those excluded regions
4. Restrict the existing task-block search to a line-index range for the `Tasks` section. This should reuse
   `is_top_level_task_line()` and `task_block_end()` so nested continuation handling stays identical.
5. Add a dedicated insertion formatter for the empty-section case, since it must guarantee one blank line after the
   heading. Do not otherwise reflow section contents.
6. Update `capture_inbox()` to use the same `insert_task_line()` path as routed captures when the inbox file already
   exists. This preserves append behavior when there is no `Tasks` section or task anchor, while honoring the new rule
   when one exists.
7. Update docs/help text in `src/native/capture.rs` and `README.md` so they describe the new placement preference and
   old fallback.

## Tests

Add focused unit coverage in `src/native/capture.rs`:

1. A file with a root `#task` before `## Tasks` and no section tasks inserts as: `## Tasks`, blank line, new task.
2. A `Tasks` section with existing top-level Bob tasks inserts after the last task block in that section, including
   indented continuation lines.
3. A later top-level Bob task outside `Tasks` does not win over the `Tasks` section.
4. A file with no `Tasks` section preserves the current global last-task fallback.
5. A `Tasks` heading inside YAML frontmatter or a fenced code block is ignored.
6. A nested heading under `Tasks` stops direct insertion, so an empty direct section inserts before that nested heading.
7. A `Tasks` header at EOF or without a trailing newline still produces one blank line before the new task.

Add or update CLI integration coverage in `tests/cli.rs`:

1. Routed capture into an existing route file with a root project task and `## Tasks` writes under `## Tasks`, not after
   the root task.
2. Existing `mac_inbox.md` with `## Tasks` gets the same placement behavior.
3. Existing routed behavior without `## Tasks` remains covered by the current tests.

## Verification

Run:

```bash
cargo fmt --check
cargo test capture
cargo test --test cli capture
```

If those pass quickly, also run:

```bash
cargo test
just install-smoke
```

## Risks

The main risk is accidentally treating non-content text as a real heading. Tracking frontmatter and fenced code blocks
mitigates that without broad parser churn. The other risk is changing whitespace more than requested in empty sections;
the implementation should only guarantee the single blank line between the `Tasks` heading and the inserted task,
leaving following content otherwise intact.
