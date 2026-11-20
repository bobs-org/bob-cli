---
keyword: Command Chip
aliases:
  - run chip
  - bob-run
---

An authored one-gesture bob run in an Obsidian note: an inline code span that
starts with `run:` (or `run[LABEL]:`) whose payload is one `bob` command line or
several joined by `&&`, or a `bob-run` fence with one command per line. Every step
starts with the word `bob`. The `bob-chips` plugin renders the span as a pill
with a solid accent ▶ run well and the fence as a titled sequence card. A click runs it
through [[command-mode]]'s engine the way `Ctrl+Enter` runs a sheet row. So does
`⌘↵` (Meta+Enter) when the chip is under the cursor, is the cursor line's only
command chip, or, on a line with no link, is its task's only one. Previews are
skipped, the binary's `confirm` commands still ask, and several steps run as one
sequence that halts at the first failure. A muted `⌘↵ Run` keycap at the end of
the cursor line marks the chip `⌘↵` would run. Shift+Click opens it in Command
Mode instead, and `Mod+Shift+Y` makes or edits one.

Payloads are literal and validated against bob's catalog: no shell, no
variables, only bob. Unknown or excluded commands render dimmed, and a click
shows bob's reason. In `mail/`, `ref/`, `ref_history/`, `.bob/`, and
`_conflicts/` notes, on mobile, or without Command Mode, a command chip only
copies its text. Running a chip never completes its task or advances the walk.

Not a [[copy-chip]], a Command Mode RECENT row, or a shell snippet. See
[Bob Chips](https://bobs-org.github.io/reference/plugins/bob-chips/#command-chips).
