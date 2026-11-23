---
keyword: Task Requirements Are Met With Their Task
aliases:
  - task reqs are met with their task
  - task requirements
  - derived req met
summary:
  "A req is a non-#task checkbox child of a task or of another req; it is met
  or moot with its owner and never written as completed; only drop and carry
  write to it."
metadata:
  status: accepted
  decided: 2026-11-21
---

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, and the vault.

**Claim.** A [[glossary:task-requirement]] ("req") is a non-`#task` checkbox
child of a task or of another req, at any depth. It is met or moot with its
owner and never written as completed. Only drop (`[-]`) and carry (`[>]`) write
to it. The successor card audits Pending reqs once, at a managed Done.
Promotion moves the boundary one level: outer reqs become `## Tasks` rows, and
their nested reqs stay reqs. A req-owning task keeps its notes in a `NOTES`
[[glossary:task-drawer]], born only when a note lands in it
(`notes_drawer_birth = with_first_note`). Capture still mirrors the task
block: a `- [ ]` bullet under a task or req is a requirement, not a note, and
authored bullets may nest beneath req chains.

**Why.** Every close path is correct by construction (capture, Ctrl+Enter,
Alt+], gcal past-day sync, collect-done, mobile, raw edits), because met is
derived from the owner. Native Markdown stays the fallback: `done/` shows
`[ ]` boxes under done tasks. Carry reuses [[glossary:task-split]]; the close
card reuses the successor-offer card. Promotion becomes correct: unmarked
children become notes even on tasks with no reqs. Rejected alternatives:

- **`TASK:` / `ALSO:` prefixes.** Extra vocabulary on every line.
- **Stored `##` hash runs.** Depth would be authored bytes instead of
  computed indent.
- **A `[req::]` field.** A second identity beside the checkbox.
- **Tick-while-working.** Met would no longer mean "the owner closed".
- **Stamping `[x]` at close.** Reopening would need a matching un-stamp, and
  unmanaged closes would drift.
- **Nested `#task` children as the feature.** Subtasks already exist; they
  carry lanes, freshness, and IDs that reqs must not have.
- **A `REQUIREMENTS` drawer that holds the reqs.** Drawers hold notes; reqs
  are checkbox children of the task.
- **Inferring reqs from `!` endings.** Silent conversion of ordinary notes.

**Cost.** Raw or GitHub views of `done/` show `[ ]` boxes under done tasks.
A managed Done with Pending reqs adds close-card ceremony. Every
any-checkbox reader and outside-text writer has to ignore or escape req
lines. Capture grammar 29 is a contract break (`!`): a `- [ ]` bullet under
a task or req is a requirement, and authored depth may pass 2 beneath a req
chain.

**Reopens when.** Bryan repeatedly wishes he could tick reqs early; an
outside consumer needs req state without the owner rule; or reqs need their
own fields.

**Evidence.** `research:202611/task_reqs_design/task_reqs_design.md`;
`research:202611/task_reqs_nested_and_notes_impact.md`;
`plan:202611/task_requirements.md`; epic `bob-cli-on`; bob-cli commits
`a0961435` (recognizer), `7cc94933` (grammar 29), `22391208` (carry),
`bf005290` (close-card contract). See also
[[decisions:capture-bullets-mirror-the-task-block]],
[[glossary:task-requirement]], [[glossary:task-drawer]],
[[glossary:task-split]], and [[glossary:successor-offer]].
