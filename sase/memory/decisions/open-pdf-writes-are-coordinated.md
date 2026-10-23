---
keyword: Open PDF Writes Are Coordinated
aliases:
  - coordinated marker writes
  - file coordination
summary:
  "bob writes Highlights PDFs only through bounded .forMerging file
  coordination around the whole per-PDF write, holding when uncertain or
  while you read; never UI automation, never .forReplacing."
metadata:
  status: accepted
  decided: 2026-10-23
---

**Applies to.** bob-cli (`ref scan`, `ref sync`, create-time ingest),
Bob Mac Capture (Refs scan presentation), the vault.

**Claim.** When `highlights.coordinate_pdf_writes` is on, every Highlights
PDF marker write goes through one bounded macOS file-coordination write
(`NSFileCoordinator`, `.forMerging`, 10 s per PDF from a 30 s scan cap)
around the whole per-PDF write sequence. macOS has Highlights save your
unsaved annotations first, bob re-checks the marker in the fresh bytes and
writes on top of them, and Highlights reloads the same tab in place. Any
uncertain branch becomes a hold — `editor_active`, `coordination_timeout`,
or `marker_changed` — with nothing written for that PDF, exit 0, and a
retry on the next scan. bob never presses keys, closes or reopens tabs,
grants itself Accessibility, or falls back to an uncoordinated write;
`.forReplacing` and option-less writes are never used. Bob Mac Capture
presents refreshes and holds from bob's scan receipt, never by reading
the vault. The key ships off until the guided acceptance test
(`scripts/dev/highlights_coordination_acceptance`) passes on the Mac.

Rejected alternatives:

- **Scripted ⌘S / close / reopen of Highlights.** Keystrokes steal focus,
  need an Accessibility grant for bob, and churn tabs.
- **`.forReplacing` or option-less writes.** Both silently drop unsaved
  annotations; only `.forMerging` asks the open document to save first.
- **Defer-while-open as the primary strategy.** Waiting for Highlights to
  go idle leaves markers stale for whole reading sessions; holds stay the
  exception (reading, blocked save, changed marker), not the rule.
- **Dropping `-w` from the cron.** The cron keeps writing through
  coordination instead of going note-only.

Evidence:
`research:202610/highlights_open_pdf_coordinated_marker_writes/highlights_open_pdf_coordinated_marker_writes.md`;
`plan:202610/highlights_coordinated_marker_writes.md`;
epic `bob-cli-at` (`coordinate`, `policy`, `executor`, `report`,
`mac-core`, `mac-app`, `accept` phases).

**Cost.** Whole-PDF holds delay annotation import by at most one scan;
a coordinated reload resets the document's undo history; the shipped
default stays off until Bryan's acceptance run passes.

**Reopens when.** The real-Highlights acceptance test shows lost
annotations or a conflict dialog (then Plan B, an app-driven "Apply
now"), or a macOS upgrade breaks file coordination.
