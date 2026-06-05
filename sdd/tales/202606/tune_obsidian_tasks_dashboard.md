---
create_time: 2026-06-05 15:07:07
status: done
source: sdd/research/202606/obsidian_improvements_consolidated.md
prompt: sdd/prompts/202606/tune_obsidian_tasks_dashboard.md
---

# Plan: Tune the Obsidian Tasks Dashboard

## Goal

Implement the first concrete "Finding 4: Tune Tasks Instead of Replacing Tasks" improvement by tightening Bryan's
existing daily Tasks dashboard. The change should preserve the inline Markdown task model and current Tasks plugin
workflow, while making the daily review list more actionable and less noisy.

The target dashboard should:

- filter by Tasks status type instead of broad `not done`;
- include actionable TODO and IN_PROGRESS tasks;
- keep dependency-blocked tasks out of the daily action list;
- group by source note path;
- show only the next few visible tasks per source note;
- preserve the existing scheduled-date, self-file, priority, sorting, short-mode, and toolbar behavior.

## Context Reviewed

- Project memory was read through:
  `sase memory read long/obsidian.md --reason "Need Obsidian vault workflow context before planning the Finding 4 task-tuning implementation"`.
- The consolidated research recommends keeping Obsidian Tasks and applying status-type filters, grouping, group limits,
  postpone behavior, and dependencies selectively.
- Local Tasks configuration in `/home/bryan/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json` has:
  - `globalFilter: "#task"`;
  - `taskFormat: "dataview"`;
  - custom `/` status type `IN_PROGRESS`;
  - custom `B` status type `ON_HOLD`;
  - custom `-` status type `CANCELLED`.
- Official Tasks docs confirm:
  - `not done` includes `TODO`, `IN_PROGRESS`, and `ON_HOLD`;
  - status-type filters support boolean combinations such as
    `( status.type is TODO ) OR ( status.type is IN_PROGRESS )`;
  - `limit groups to <N> tasks` is the supported per-group cap once a query has `group by`;
  - the postpone button is hidden only when `hide postpone button` is used.
- The live daily template is `/home/bryan/bob/_templates/daily.md`.
- Today's daily file is `/home/bryan/bob/2026/20260605_day.md`.
- `/home/bryan/bob/_templates/daily.md` is tracked and clean.
- `/home/bryan/bob/2026/20260605_day.md` already exists but is untracked, with Pomodoro content not created by this
  task. Because the vault instructions require staging and committing only task-related changes, I should not
  automatically add that whole untracked note just to change its query.

## Implementation Scope

Primary implementation:

- Update only `/home/bryan/bob/_templates/daily.md` in the first pass.
- Replace the first Tasks query line:

```tasks
not done
```

with:

```tasks
( status.type is TODO ) OR ( status.type is IN_PROGRESS )
```

- Add the group cap after the existing `group by path` instruction:

```tasks
limit groups to 3 tasks
```

- Leave the existing `is not blocked` line in place. This keeps dependency-blocked tasks out of the daily action queue,
  while the status-type line excludes `ON_HOLD`/`B` tasks that are not actionable.
- Leave `hide toolbar` unchanged and do not add `hide postpone button`, so Tasks can still show postpone controls where
  it normally would.
- Leave `delete` on completion and task IDs/dependencies out of scope. The research recommends those only for bounded
  workflows, and this task does not identify a specific workflow that warrants them.
- Do not add a new CLI command, Rust code, or broad vault migration. The CLI rules memory does not need to be read
  unless the implementation scope changes to add or alter CLI behavior.

Conditional current-day handling:

- Re-check `/home/bryan/bob/2026/20260605_day.md` before implementation.
- If it has become tracked and clean, apply the same query update there.
- If it is still untracked, do not edit or commit it automatically. Report that today's existing note still has the old
  dashboard and that updating it would require either user approval to include the whole note in a vault commit or a
  later edit after the note becomes tracked.

## Vault Safety

Before editing under `/home/bryan/bob`:

- inspect `git -C /home/bryan/bob -c color.status=false status --short -- _templates/daily.md 2026/20260605_day.md`;
- edit only the selected file(s);
- inspect the focused diff afterward;
- do not stage, revert, or overwrite unrelated existing vault changes.

After any `/home/bryan/bob` file change:

- commit the vault change using the required `/sase_git_commit` workflow;
- stage only the file(s) changed for this task;
- use a narrow commit message such as `Tune daily Tasks dashboard`.

## Verification

Focused verification is enough because this is Markdown Tasks query configuration, not Rust behavior:

- confirm `_templates/daily.md` has the new status-type line and `limit groups to 3 tasks`;
- confirm the existing self-file, scheduled-date, priority-field, grouping, sorting, `short mode`, and `hide toolbar`
  lines remain intact;
- confirm the focused vault diff contains only the intended Tasks query change;
- do not run the full Rust test suite unless Rust code unexpectedly changes.

Optional manual check after Obsidian has synced/rendered:

- open a new daily note and verify the Tasks dashboard shows TODO and IN_PROGRESS tasks, excludes ON_HOLD/blocked work,
  groups by path, and caps each path group at three visible tasks.
