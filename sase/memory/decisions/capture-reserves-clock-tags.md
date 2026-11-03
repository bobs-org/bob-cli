---
summary: "Free clock tags at capture item edges are task tags, not types."
metadata:
  status: accepted
---

*Accepted · project*

aka capture clock tags, reserved clock spellings

**Applies to.** bob-cli capture and task-type configuration.

**Claim.** Free `#off`, `#on`, `#offclock`, `#off-clock`, and `#untracked` at either capture-item edge are task tags, never type markers. New-task capture preserves these tokens verbatim and offers none as types. Capture refuses an edge clock tag on an item that selects an existing task, because it cannot write that tag onto the task line. Type configuration and type rename/split reject all five spellings as names.

**Why.** The same explicit clock vocabulary must work in Calendar, Obsidian task lines, and capture without requiring an escape. Capture already distinguishes free edge `#<type>` markers from prose, so the reserved spellings are the narrow exception to that rule.

Rejected alternatives: requiring `\\#off` to create the tag, or introducing a `%` directive for clock selection.

**Evidence.** `plan:202611/off_on_tags_and_offclock_areas.md`, phase `capture-clock-tags`; bob-cli capture clock-tag implementation.

**Cost.** The five names cannot be used as task types, and capture cannot add a clock tag to an existing task; edit that task line directly.

**Reopens when.** Bryan changes the clock-tag vocabulary or asks capture to edit tags on existing tasks.

Supersedes in part [[decisions:capture-sigils-say-where-what-how]]: this is the narrow exception for free edge clock tags.
