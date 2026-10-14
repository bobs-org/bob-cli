---
title: Capture `*` Blocks Existing Tasks
summary: Capture `*` makes the new task a prerequisite of an existing task.
requested: project
aka:
  - capture star blocks
  - blocked-task capture
---

**Applies to.** bob-cli and Bob Mac Capture.

**Claim.** A whitespace-delimited `*note:block-id` capture modifier means
that the new or explicitly selected task blocks the named existing task.
The edge is stored only on the target task's managed Depends-On line;
the new task's BLOCKS line is its generated mirror. `*` is recognized only
in the documented leading and trailing modifier runs, with guards for
Markdown emphasis, list markers, multiplication, park operators, and
escaped literals. A bare `*` opens completion only as the final token after
other text on the same line. Capture consumes the target's live Task Links
by resolved path and block ID, preserves authored child notes as plain text,
and deletes only Pomodoro entries this capture empties. It reports consumed
links and removed entries. A newly captured blocker inherits the target's
commitment up to Next; it is linked before the target's Task Links are
consumed. Existing owners and `&` plus `*` on one item are supported.
Capture never auto-links the new task into the target's Pomodoro.

**Why.** The prerequisite is often discovered while its blocked task is
already in view, so a two-item draft and hand-invented block ID add avoidable
steps and cannot reach nested notes. `*` is the direct visual dual of `&`,
and the existing Depends-On writer can retain the single-line source of
truth. Rejected alternatives were a Task Card follow-up, directional `&`, a
`blocks:` keyword, a Mac-only toggle, authoring BLOCKS as source data,
automatic linking into the target's vacated slot, and other sigils. The
picker and preview remain renderers of bob's parse, completion, and capture
contracts.

**Cost.** Capture can edit a task note and today's ledger that the user did
not name directly, including deleting a running Pomodoro when its last link
is consumed. Capture's preservation behavior intentionally differs from
reconcile and plugin PDL7 behavior. The block is soft: a later capture link
gesture can drop it. Bob and Bob Mac Capture ship on separate release trains.

**Reopens when.** Bryan changes the `*` grammar or direction, asks for
automatic linking into the target's session, wants reconcile or plugin
consumption to preserve notes the same way, or replaces the Depends-On line
as the source of truth.

**Evidence.** `research:202610/capture_star_blocks_modifier/capture_star_blocks_modifier.md`;
`plan:202610/capture_star_blocks.md`; bob-cli commits `04a6e84` (contract),
`1a074c6` (consumption), `8774e89` (parser), `158d6de` (writer),
`78f1a69` (completion), and `1b9d35f` (execution); Bob Mac Capture commit
`f2a5cb5` (direction-aware picker and preview); publish phase `bob-cli-7n.8`.
See also [[decisions:task-deps-are-depends-on-links]] and
[[decisions:capture-link-drops-blocking-deps]].
