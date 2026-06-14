---
create_time: 2026-06-14 08:33:06
status: done
prompt: sdd/prompts/202606/move_bob_tasks_to_bob_v1.md
---
# Plan: Move bob.md Tasks into bob_v1.md

## Objective

Move task blocks from `/home/bryan/bob/bob.md` into `/home/bryan/bob/bob_v1.md` according to priority:

- Task blocks whose parent task line has an inline P1 field (`[p::1]` or spacing-equivalent `[p:: 1]`) become normal
  bullets under a new bottom-level `## Future Work` section in `bob_v1.md`.
- All remaining task blocks from `bob.md` move into the existing `## Tasks` section of `bob_v1.md`.
- `bob.md` keeps its frontmatter, title, and prose, but no longer keeps the moved task blocks.

## Current Observations

- `/home/bryan/bob/bob.md` currently has a `## Tasks` section with 30 parent task blocks.
- 19 parent task blocks are P1 when `[p::1]` and `[p:: 1]` are treated as the same Dataview-style inline field.
- 11 parent task blocks are non-P1 and should remain tasks in `bob_v1.md`.
- Some task blocks have indented child bullets; those children should move with their parent block.
- `/home/bryan/bob/bob_v1.md` already has:
  - one `^prj` project lifecycle task before `## Tasks`, which should stay where it is;
  - a placeholder task under `## Tasks`, which should be removed when inserting the moved real tasks;
  - a `## Project Notes` section, after which the new `## Future Work` section should be appended.
- `task_count` and `open_task_count` are machine-maintained by the project workflow, so this migration should not
  manually recalculate or rewrite those frontmatter fields.
- `git -C /home/bryan/bob status --short` shows existing user work, including a modified `bob.md` and untracked
  `bob_v1.md`; preserve current content and do not revert unrelated vault changes.

## Transformation Rules

1. Treat a task block as one checkbox parent line plus any immediately following indented child lines.
2. Classify parent task lines matching `- [ ] #task ...` or `- [x] #task ...`.
3. P1 classification uses `\[p::\s*1\]` so `[p::1]` and `[p:: 1]` both count.
4. For P1 task blocks moved to `## Future Work`:
   - replace the parent line's leading task syntax (`- [ ] #task ` or `- [x] #task `) with a normal bullet prefix
     (`- `);
   - remove the P1 inline field;
   - preserve task text, created/completion metadata, block IDs such as `^workspaces`, and indented child bullets.
5. For non-P1 task blocks moved to `## Tasks`:
   - preserve the original task syntax, metadata, completion state, block IDs, and child bullets.
6. Do not normalize quote characters, frontmatter ordering, blank lines outside the touched sections, or unrelated vault
   files.

## Implementation Steps

1. Re-read both target files immediately before editing to avoid working from a stale snapshot.
2. Build the migrated content in a temporary mental/scripted pass:
   - extract all task blocks from `bob.md`;
   - split them into P1 blocks and remaining blocks;
   - render P1 blocks as normal bullets for `Future Work`.
3. Edit `bob.md` so its `## Tasks` section no longer contains the moved task blocks. Leave the section heading in place
   unless removing it is clearly cleaner after inspecting the final content.
4. Edit `bob_v1.md`:
   - replace the placeholder task inside `## Tasks` with the 11 remaining task blocks;
   - append `## Future Work` at the bottom and add the 19 converted P1 bullet blocks.
5. Preserve the existing project lifecycle task before `## Tasks`.

## Verification

1. Run targeted greps to confirm:
   - no checkbox task lines remain in `bob.md`;
   - no `[p::1]` or `[p:: 1]` task lines remain in `bob.md`;
   - `bob_v1.md` contains the migrated non-P1 tasks under `## Tasks`;
   - `bob_v1.md` contains a bottom `## Future Work` section whose items are normal bullets, not `#task` checkbox tasks.
2. Inspect the final numbered files around the edited sections.
3. Run `git -C /home/bryan/bob diff -- bob.md bob_v1.md` to review only the target-note changes.
