---
keyword: Bulk Capture
aliases:
  - "multi-item capture"
  - "batch capture"
---

A bulk capture holds multiple items: blank-line items, bare-URL lists, chained session operators, or one item per row from a `@@template:` draft. Template rows are later nonblank physical lines, one capture per line, not blank-line-separated items; capturing the draft is exactly capturing its `bob capture -E` expansion. Blank or whitespace-only lines separate items; a blank-line-free block of two or more
bare-URL lines splits into one item per URL, and a line chaining session operators
(`+2 =x`, `=x =`) into one item per operator. Authored child bullets never start an
item. Each item is still judged alone, with its own markers and whole-item forms, so
bulk linking (several `@route:id` items) and bulk completion (one `!note:id` per item)
work. The draft-wide `@@route` form routes otherwise-unrouted items across a blank-line batch; the draft-wide `@@template:` form declares and expands before parsing. One staged planner
(`CaptureBatchPlanner`) runs the items in order, so each item sees the edits planned
before it (insertion points, block-ID checks, Pomodoro lineup numbers) exactly as
sequential captures would, and any item's failure rolls the whole draft back. JSON
output puts the first result in the legacy top-level fields plus an ordered `captures`
array (`capture-parse` adds `items`), and human output numbers items `1/N`. Bob Mac
Capture still submits one `bob capture` call: the preview stacks items under a count,
the footer stays **Capture** instead of a single-item verb such as Link, Start, Close,
Complete, or Queue, the notification is titled with the count, and Shift-Return in the
`!` picker appends a blank line plus `!` for the next completion. Docs and code also say
"batch" for any capture's staged writes, even a single item's.
