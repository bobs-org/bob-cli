---
create_time: 2026-07-08 13:06:49
status: wip
prompt: sdd/prompts/202607/fix_next_tasks_query_operator.md
---
# Plan: Fix the broken "NEXT Tasks" query in `dash.md` (invalid `status.name is` operator)

## Problem

The **NEXT Tasks** section of `~/bob/dash.md` renders an error instead of a task list:

```
Tasks query: do not understand query
Problem line: "status.name is Next"
```

The Obsidian Tasks plugin cannot parse the query, so the whole block is replaced by the error box (screenshot:
`.sase/home/tmp/screenshots/20260708_130208.png`).

## Root cause (confirmed from the plugin source)

The offending line is line 32 of `~/bob/dash.md`:

```tasks
status.name is Next
```

The Tasks plugin (installed version **8.0.0**, `~/bob/.obsidian/plugins/obsidian-tasks-plugin/main.js`) treats
`status.name` and `status.type` as **two different kinds of field** with **different operator grammars**:

- **`status.type`** is an _enum_ field, implemented by a dedicated class (`StatusTypeField`) whose grammar is
  `status.type (is | is not) (TODO | IN_PROGRESS | DONE | CANCELLED | NON_TASK)`. This is why the sibling WIP query
  (`status.type is IN_PROGRESS`) and READY query (`status.type is TODO`) work.
- **`status.name`** is a _text_ field, implemented as `StatusNameField extends` the shared TextField base class. That
  base class only accepts the operators `includes | does not include | regex matches | regex does not match`.

So `is` is **not a valid operator for `status.name`** — it exists only on `status.type`. When the plugin can't match a
line against any known filter grammar, it emits "do not understand query" and fails the entire code block (not just the
one line). `status.symbol` was also checked and is **not** a queryable field in this version, so it is not an option.

This bug was introduced by the approved `replace_blocked_with_next_task_status` plan, whose design decision #2 specified
`status.name is Next` as the NEXT query's status line — a valid-looking but ungrammatical operator/field combination
that was never rendered in Obsidian before being committed.

## Fix (single line)

Change **line 32** of `~/bob/dash.md` from:

```
status.name is Next
```

to a grammatically valid line that selects only the Next status.

**Recommended:**

```
status.name includes Next
```

Rationale: it preserves the approved plan's stated intent (match by the status _name_ "Next"), reads naturally next to
the sibling `status.type is …` lines, and is unambiguous with the current status set — `Next` is the **only** status
name containing the substring "next" (the full status list is `Todo`, `In Progress`, `Done`, `Next`, `Canceled`), so the
case-insensitive substring match resolves to exactly the `*` status.

Nothing else in the NEXT block changes — the remaining lines are byte-for-byte identical to the working WIP/READY
queries, so this one operator is the whole fix.

### Alternatives (equally valid — decide during review)

1. **`status.type is ON_HOLD`** — mirrors the sibling queries' grammar _exactly_ (same `status.type is <TYPE>` form), is
   guaranteed to parse, and is an exact enum match. Works because `Next` is currently the only `ON_HOLD` status.
   Trade-off: it selects by _type_, not name, so it would also match any future `ON_HOLD` status; it is slightly less
   self-documenting than naming "Next".
2. **`status.name regex matches /^Next$/i`** — airtight exact-name match (no substring/case looseness). Trade-off: more
   verbose and less readable than `includes`.

The recommended `status.name includes Next` is the smallest change most faithful to the original intent; the two
alternatives are noted only in case exact-grammar-parity or exact-match strictness is preferred.

## Expected result after the fix (set expectations)

- The error box disappears; the NEXT Tasks section renders as a normal Tasks query block.
- The section will show **0 tasks** for now. This is **not** a regression: a grep of the vault (excluding `_templates`)
  found **zero** `[*] #task` lines, so there are legitimately no Next tasks to display yet. The section will populate as
  soon as a task is marked `[*]` with the `#task` filter (and is unscheduled or scheduled on/before today, not `#hide`,
  and not dependency-blocked — same visibility rules as the sibling sections).

## Verification

1. Reload Obsidian (or reopen `dash.md` with `ob`) so the Tasks plugin re-parses `dash.md`.
2. Confirm the **NEXT Tasks** section renders a normal (empty) Tasks block with **no** "do not understand query" error.
3. Optional live check: temporarily add `- [*] #task try next thing` to a non-`_templates` note (no `#hide`, scheduled
   today or undated), confirm it appears under **NEXT Tasks** and **not** under WIP/READY, then remove it.

## Scope / out of scope

- **In scope:** exactly one line in `~/bob/dash.md` (the vault is edited directly; it is not a numbered-workspace linked
  repo). No plugin, CSS, `data.json`, or bob-cli changes are needed — those surfaces from the prior migration are all
  correct; only the dashboard query operator was wrong.
- **Optional doc-consistency (not required):** the approved SDD record
  `sdd/tales/202607/replace_blocked_with_next_task_status.md` documents `status.name is Next` (design decision #2 and
  the example block). It is a point-in-time "done" record; leaving it as-is is fine, but if desired its query example
  can be corrected to the chosen operator to avoid re-propagating the mistake.
- **Out of scope:** the dependency-based `is not blocked` filter (unrelated, retained), and any change to how tasks are
  authored/cycled.

## Rollout notes

- `~/bob` is a git repo; `dash.md` is tracked there. The Tasks plugin reads `dash.md` live — apply the edit with
  Obsidian closed or reload afterward so the change is picked up (this file is Markdown content, not plugin config, so a
  simple note reload suffices).
- Committing `~/bob` is left to the normal per-repo commit flow and is not part of implementation unless requested.
