---
keyword: Capture
---

One `bob capture` run over one draft: its arguments, piped stdin, or Bob Mac Capture's
panel, which submits the whole draft as exactly one `bob capture` call and hides only
once bob reports success (see [[decisions:mac-capture-is-a-thin-client]]). bob splits
the draft into capture items, each a parent line plus optional authored child bullets,
and each item's markers decide what it does. That is far more than GTD inbox capture: an
item may create a task (default `mac_inbox.md`, an [[inbox-file]]), bullet, sub-bullet,
or project note; queue a bare public URL as a background reference job, never touching
the network; link, start, complete, or relate existing tasks (`@route:id`, `!note:id`,
`&`, `*`); or start, adjust, shift, or close today's Pomodoro (`=`, `+N`, `++N`, `x`).
bob plans every edit against in-memory snapshots of the target notes and today's daily
note, then writes all or nothing: a note changed on disk since planning, or any failed
item, aborts the run and rolls back anything already written. `--dry-run` runs the same
planner without writing; the app's live preview is `bob capture --dry-run --no-clip`. An
unfinished picker query (`:dee`, `/fix`, `!fix`, `&`, `Call the bank +`) is refused,
never captured as prose. The editor-facing `capture-*` companion commands are not
captures, nor are `bob gkeep pull`, `bob ref create`, or Obsidian keymaps. A draft with
two or more items is a [[bulk-capture]]; the grammar lives in `docs/capture.md`.
