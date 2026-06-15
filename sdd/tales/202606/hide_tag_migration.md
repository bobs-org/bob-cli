---
create_time: 2026-06-15 10:48:20
status: wip
prompt: sdd/prompts/202606/hide_tag_migration.md
---
# Plan: Migrate main project/reference task hiding from `[p::2]` to `#hide`

## Context

`bob projects` currently uses the presence of a `[p::...]` inline field on the main `^prj` task as the hide/surface
marker:

- active projects with no visible work and no open sub-projects have `[p::2]` removed so the `^prj` task appears in the
  Tasks query;
- active projects with other visible work or open sub-projects get `[p::2]` added back so the `^prj` task is hidden;
- `bob projects list` reports an open `^prj` task with a `p` field as `open` and one without it as `on dash`.

`bob highlights` currently emits generated PDF reference tasks as:

```md
- [ ] #task [[lib/...pdf]] [p::2] ^ref
```

The vault has existing main-note tasks to migrate:

- `^prj` tasks in project notes and `_templates/new_project.md`, including one spacing variant `[p:: 2]`.
- generated `^ref` PDF tasks under `~/bob/ref/**`.
- one generated `^ref` task currently has duplicate `[p::2]` fields; the migration should remove both and add a single
  `#hide`.

The active dashboard Tasks query in `~/bob/dash.md` still filters out `[p::N]` via `task.originalMarkdown`. Several
previous daily notes also contain that same query. However, as of this inspection, `~/bob/_templates/daily.md` and
today's daily note `~/bob/2026/20260615.md` do not contain the old Tasks query block. The implementation should still
verify those requested files before editing and should update the active query in `dash.md` unless the user narrows
scope.

## Desired Behavior

`#hide` becomes the hide/surface marker for generated/main project and reference tasks.

- A hidden main project task is represented as `- [ ] #task ... #hide ^prj`.
- A surfaced main project task has no `#hide` tag and can appear in the Tasks query.
- A generated main reference task is represented as `- [ ] #task [[lib/...pdf]] #hide ^ref`, preserving
  completion/cancelled metadata before the final block ID.
- The Tasks query should stop mentioning `p` and should exclude `#hide` tasks instead.
- Other uses of `[p::N]` in ordinary tasks are not migrated unless they are the special `^prj` or `^ref` lines covered
  by `bob projects` / `bob highlights`.

## Implementation Steps

1. Update `bob projects` semantics in `src/native/projects.rs`.
   - Replace `PROJECT_TASK_SHAPE` with a `#hide` example.
   - Replace `PrjTask.priority: Option<String>` with a boolean/tag-focused field such as `hidden: bool`.
   - Rename the planning changes from priority operations to hide-tag operations, for example `AddHideTag` and
     `RemoveHideTag`.
   - Change planning so the same surface/hide decision is based on the `#hide` tag:
     - remove `#hide` from an open active `^prj` task when there are no non-hidden open tasks and no open sub-projects;
     - add `#hide` to an open active `^prj` task when there are non-hidden open tasks or open sub-projects.
   - Change the project task count used by sync from "open tasks without `[p::...]`" to "open tasks without `#hide`",
     excluding the `^prj` task itself.
   - Keep `scheduled` cleanup behavior unchanged for `^prj` tasks.
   - Make tag add/remove helpers preserve the trailing `^prj` block ID and handle whitespace/CRLF similarly to the
     existing inline-field edit helpers.

2. Update `bob highlights` generation and parser messaging in `src/native/highlights_ref/mod.rs`.
   - Replace `PDF_TASK_PRIORITY` with a `PDF_TASK_HIDE_TAG` constant.
   - Generate new reference notes with `#hide` before `^ref`.
   - Keep `parse_pdf_task_line()` backward-compatible with legacy `^ref` lines that still have `[p::2]`, no hide tag, or
     completion/cancelled fields.
   - Update malformed-task help text and tests to describe `#hide`.
   - Preserve marker-only checkbox rewrites exactly; they should keep whatever non-checkbox tokens are present on the
     task line.

3. Update tests and docs.
   - Rewrite `tests/cli.rs` expectations for generated `^prj` and `^ref` lines from `[p::2]` to `#hide`.
   - Update project sync tests for add/remove `#hide`, idempotency, CRLF, sub-project behavior, and status-only changes.
   - Update `docs/projects.md` and `docs/highlights-ref-sync.md` to document `#hide` rather than `[p::2]`.
   - Leave historical SDD/tale files alone unless a test or current user-facing doc depends on them.

4. Migrate `~/bob/` note content carefully.
   - For each task line containing `^prj` or `^ref`, remove all `[p::2]` / `[p:: 2]` fields and add one `#hide` token if
     it is not already present.
   - Place `#hide` before the trailing block ID, and for `^ref` tasks place it after the PDF wikilink and before
     completion/cancelled metadata when practical.
   - Update `~/bob/_templates/new_project.md` from `[p::2]` to `#hide`.
   - Do not modify unrelated `[p::N]` tasks.
   - Be careful with the existing dirty vault worktree and avoid reverting unrelated user changes.

5. Update Tasks queries.
   - In each requested query file that contains the old `p` filter, replace:
     ```tasks
     filter by function !/(^|[^\[])\[p::\s*\d+\s*\](?!\])/.test(task.originalMarkdown)
     ```
     with a `#hide` exclusion, preferably:
     ```tasks
     filter by function !task.tags.includes("#hide")
     ```
   - Re-check `~/bob/_templates/daily.md` and `~/bob/2026/20260615.md`; if they still have no Tasks query, record that
     no query edit was possible there.
   - Update `~/bob/dash.md` because it currently owns the active Tasks query with the old `p` filter.
   - Consider whether to update prior generated daily notes that still contain the old query; do this only if scope
     remains "all existing query copies" rather than only template/today/dashboard.

## Verification

- Run focused Rust tests for `projects` and `highlights` behavior, then the broader CLI test suite if runtime is
  reasonable:
  - `cargo test projects`
  - `cargo test highlights_ref`
  - `cargo test --test cli`
- Run `cargo fmt`.
- Use `rg` checks in the repo and vault:
  - no current generated `^prj` / `^ref` examples still require `[p::2]`;
  - no migrated `^prj` / `^ref` task line under `~/bob` still has `[p::2]`;
  - migrated hidden `^prj` / `^ref` task lines have exactly one `#hide`;
  - requested query files no longer reference `p` in their Tasks filters.
- Run `bob projects sync --dry-run -b ~/bob` after the code change to confirm the new `#hide` logic is idempotent or
  only reports expected project state changes.
- Review `git diff` for both this repo and `~/bob` before summarizing.

## Risks and Mitigations

- `#hide` must not land after the final block ID, because block links are expected to resolve to the task line.
  Add/remove helpers should insert immediately before `^prj` / `^ref`.
- The Tasks plugin tag API may differ across versions. If `task.tags.includes("#hide")` does not work in Obsidian, fall
  back to a raw Markdown token filter such as `!/(^|\\s)#hide(\\s|$)/.test(task.originalMarkdown)`.
- The vault is already dirty, including several reference notes. Migration should be line-targeted and diffs should be
  reviewed rather than normalized wholesale.
- Existing legacy notes in other vaults may still have `[p::2]`; parsers should accept them even though new rendering
  and Bryan's vault migration use `#hide`.
