---
aliases:
  - true-case drawers
  - drawers keep their names
keyword: Drawers Keep Their Real Names
summary: Drawer titles stay verbatim in bold; ALL-CAPS is no longer structure.
metadata:
  status: accepted
---

# Drawers Keep Their Real Names

**Applies to.** bob-cli, bob-plugins, Obsidian, and Bob Mac Capture.

**Claim.** A task drawer is a direct, non-checkbox child bullet whose whole title is
bold, such as `- **Future Work**`. The title stays exactly as authored on disk, in
capture, in Obsidian, and through project-note promotion, reversal, rename, and Mac
rendering. Drawer identity remains the case-folded title key, so capitalization changes
the displayed spelling but not identity. Capture shortcuts resolve to the canonical
bold header. ALL-CAPS recognition is retired; an ALL-CAPS bullet is ordinary content.

**Why.** The marker separates a drawer from a task checkbox while letting section names
keep their real spelling. Retiring capitalization as structure avoids false drawers
and makes the representation lossless through promotion and reversal.

**Cost.** Drawer Markdown requires the bold marker. Legacy ALL-CAPS lines must be
renamed or released before cutover, and readers that recognize drawers must ship before
writers emit the new form.

**Reopens when.** A drawer needs a representation that cannot be expressed by one bold
title bullet, or Bryan chooses a different drawer marker.

**Evidence.** `plan:202611/true_case_drawers.md`; phases `bob-cli-ph.10` (retired
ALL-CAPS recognition) and `bob-cli-ph.8` (Mac rendering and typing). See
[[glossary:task-drawer]] and [[decisions:capture-bullets-mirror-the-task-block]].
