---
create_time: 2026-07-07 17:16:55
status: wip
prompt: sdd/prompts/202607/wip_tasks_dashboard.md
---
# Plan: Add WIP Tasks to Dashboard

## Context

`~/bob/dash.md` is the Obsidian dashboard note. It currently has a single `## Tasks` section containing one Obsidian
Tasks query:

```tasks
folder does not include _templates
( status.type is TODO ) OR ( status.type is IN_PROGRESS )
is not blocked
filter by function task.file.path !== query.file.path
filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), "day")
filter by function !task.tags.includes("#hide")
group by path
sort by function task.file.path
sort by function task.lineNumber
short mode
hide toolbar
```

The requested change is to split the dashboard view into a focused WIP section followed by the broader all-task section.

## Implementation

1. In `~/bob/dash.md`, insert a new `## WIP Tasks` section above the existing tasks section.
2. Add a single Obsidian Tasks query to `## WIP Tasks`.
3. Keep the WIP query identical to the existing query except for the status predicate:

   ```tasks
   status.type is IN_PROGRESS
   ```

   This preserves the same folder exclusion, blocked-task exclusion, self-file exclusion, schedule filter, `#hide`
   filter, grouping, sorting, short mode, and toolbar hiding behavior.

4. Rename the existing `## Tasks` heading to `## All Tasks`.
5. Leave the existing all-task query otherwise unchanged, including its current status predicate:

   ```tasks
   ( status.type is TODO ) OR ( status.type is IN_PROGRESS )
   ```

## Verification

1. Re-read `~/bob/dash.md` after editing and confirm the section order is:
   - `## WIP Tasks`
   - `## All Tasks`
   - `## Projects ([[projects.base]])`
   - `## Reading List ([[refs.base]])`
2. Confirm there are exactly two `tasks` query fences.
3. Confirm the WIP query differs from the all-task query only in the status predicate.

## Risk

The main risk is using a status predicate that does not match the existing Tasks plugin syntax. This plan avoids that by
reusing the already-present `status.type is IN_PROGRESS` expression from the current dashboard query.
