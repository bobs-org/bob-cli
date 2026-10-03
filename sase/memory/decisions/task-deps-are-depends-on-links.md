---
keyword: Task Dependencies Are Links On One Depends-On Line
aliases:
  - task dependency links
  - depends-on line
  - dep links
summary:
  "A task's prerequisites live as plain task dependency links on one managed
  Depends-On first-child line; that line is the source of truth and the
  [dependsOn::] / [id::] fields are derived from it."
metadata:
  status: accepted
  decided: 2026-10-03
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** A task's prerequisites live as plain, never-transcluded [[glossary:task-link]]s
on one managed `⛓️ **DEPENDS ON:**` first-child line. That line is the source of
truth: `bob task-status-hooks` derives the task's `[dependsOn::]` field and each
target's `[id::]` from it through R1–R10 reconciliation, and the task stays Blocked
`[?]` while any prerequisite is open. Linking the dependent under today's Pomodoros
raises its open prerequisites to Next; closing the dependent never closes its
targets. `docs/task-dependencies.md` owns the grammar, identity, link form,
reconciliation, and chip rules.

**Why.** Dependencies used to be inferred from any sole transcluded child, so the
Ctrl+Shift+P picker only saw the current note (44 of 45 edges were same-note),
`#^ref` reading embeds promoted tasks they merely quoted, closing an embedded
ledger link recursively closed its prerequisites, Ctrl+D on `dependsOn` orphaned
the embeds, and picker writes took several undo steps. One managed line with
plain links removes the inference: the picker fuzzy-searches every open task in
the vault, each write commits in one transaction, and live chips render each
link's status. Rejected alternatives:

- **Links only.** No derived field, so Tasks queries and Blocked derivation lose
  their whole-vault index.
- **Fields only with a virtual line.** Nothing readable or hand-editable in the
  note; hand edits have nowhere to land.
- **A field-authoritative generated line.** Writers would have to update the field
  and regenerate the line in lockstep; the line could never be the edit surface.
- **One bullet per dependency.** Unbounded children bloat the task block and
  collide with logs and notes.
- **Links on the task line.** No room for several prerequisites; the task line is
  already crowded with fields.
- **The Tasks modal as the editor.** It cannot be made a real editing surface, so
  its dependency edits to a task that has a line are dropped with a warning.
- **Stored aliases and strikes.** They break the canonical writer form and the
  chip model; reader tolerance plus canonicalisation covers hand variants.

**Cost.** Four parsers kept in sync by shared vectors (Rust, nav, ledger-tools,
cycler/block-id-prompt recognisers); the projection exists at all; deleting the
whole line outside Obsidian is re-adopted rather than honoured; Tasks-modal
dependency edits to a task that has a line are dropped; `#^ref` embeds no longer
promote.

**Reopens when.** R1/R2 warnings appear on most hooks runs for two weeks; the
Tasks modal becomes a real editing surface; Tasks' `is blocked` stops being
load-bearing.

Evidence: `research:202610/task_dep_link_depends_on_line/task_dep_link_depends_on_line.md`;
epic `bob-cli-3n` (`plan:202610/task_dep_links.md`); `docs/task-dependencies.md`
(`f4b0d12`); hooks parser and edges (`2d4d508`); R1–R10 reconciliation (`043d9c5`).
